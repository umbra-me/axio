//! A provider speaking the OpenAI chat-completions dialect.
//!
//! This exists to answer a question the design could not answer by inspection:
//! is the `Provider` trait actually provider-shaped, or is it the Messages API
//! wearing a trait? The answer turned out to be mostly the former, with one
//! real seam — see `wire_to_openai`, where a single user message carrying N
//! tool results has to become N separate messages. That conversion belongs in
//! the provider, which is the point of having the seam at all.
//!
//! * **Effort** is sent as `reasoning_effort`. It was dropped here for a while
//!   on the belief that the dialect had no equivalent; the endpoint settled it
//!   by rejecting an invalid value with a 400 that named the five it accepts,
//!   while ignoring a field it had genuinely never heard of. Only four survive
//!   the crossing — see `reasoning_effort` for which two collapse and why.
//! * **Reasoning blocks** have no equivalent, so they are not replayed. That
//!   is safe rather than lossy: the transcript keeps them, and the projection
//!   already drops blocks minted by a different model.

mod request;
mod stream;

pub use request::build_body;
pub use stream::OpenAiStream;

use std::pin::Pin;

use axio_core::protocol::Usage;
use axio_core::provider::{
    BlockKind, BoxStream, Effort, ModelInfo, ModelRequest, Provider, ProviderError, Role,
    StopReason, StreamEvent, WireContent,
};
use axio_core::redact::{Redacted, register_secret};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::client::transport_error;
use crate::sse::SseDecoder;

/// Ollama's hosted endpoint. Any other host speaking the same dialect works
/// too; that is the whole appeal of implementing this one.
pub const OLLAMA_BASE: &str = "https://ollama.com/v1";

pub struct OpenAiProvider {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    id: String,
}

impl OpenAiProvider {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        crate::client::install_crypto_provider();
        let api_key: String = api_key.into();
        register_secret(api_key.clone());

        let http = reqwest::Client::builder()
            .user_agent(concat!("axio/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ProviderError::Transport(Redacted::new(e.to_string())))?;

        Ok(Self {
            http,
            api_key,
            base_url: base_url.into(),
            id: id.into(),
        })
    }

    pub fn ollama(api_key: impl Into<String>) -> Result<Self, ProviderError> {
        Self::new(api_key, OLLAMA_BASE, "ollama")
    }
}

/// Unknown and unpriced. Reporting zero is honest: a made-up price would make
/// the budget check silently wrong rather than visibly absent. `--doctor` says
/// so out loud rather than printing another provider's table.
pub fn model_info(_model: &str) -> ModelInfo {
    ModelInfo {
        context_window: 128_000,
        max_output_tokens: 32_000,
        input_price: 0.0,
        output_price: 0.0,
        cache_read_price: 0.0,
        cache_write_price: 0.0,
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn model_info(&self, model: &str) -> ModelInfo {
        model_info(model)
    }

    async fn stream(
        &self,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        use futures_core::Stream;
        use std::task::{Context, Poll};

        let body = build_body(&req);
        let url = format!("{}/chat/completions", self.base_url);

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            r = self.http
                .post(&url)
                .bearer_auth(&self.api_key)
                .header("content-type", "application/json")
                .json(&body)
                .send() => r.map_err(|e| transport_error(e, &self.base_url))?,
        };

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body = response.text().await.unwrap_or_default();
            return Err(crate::anthropic::classify(
                status,
                retry_after.as_deref(),
                &body,
            ));
        }

        struct Body {
            inner: Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
            decoder: OpenAiStream,
            pending: std::collections::VecDeque<StreamEvent>,
            cancel: CancellationToken,
            finished: bool,
        }

        impl Stream for Body {
            type Item = Result<StreamEvent, ProviderError>;

            fn poll_next(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                loop {
                    if let Some(ev) = self.pending.pop_front() {
                        return Poll::Ready(Some(Ok(ev)));
                    }
                    if self.finished {
                        return Poll::Ready(None);
                    }
                    if self.cancel.is_cancelled() {
                        self.finished = true;
                        return Poll::Ready(Some(Err(ProviderError::Cancelled)));
                    }
                    match self.inner.as_mut().poll_next(cx) {
                        Poll::Ready(Some(Ok(chunk))) => match self.decoder.push(&chunk) {
                            Ok(events) => self.pending.extend(events),
                            Err(e) => {
                                self.finished = true;
                                return Poll::Ready(Some(Err(e)));
                            }
                        },
                        Poll::Ready(Some(Err(e))) => {
                            self.finished = true;
                            return Poll::Ready(Some(Err(ProviderError::Transport(
                                Redacted::new(e.to_string()),
                            ))));
                        }
                        Poll::Ready(None) => {
                            self.finished = true;
                            match self.decoder.finish() {
                                Ok(events) => self.pending.extend(events),
                                Err(e) => return Poll::Ready(Some(Err(e))),
                            }
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
            }
        }

        Ok(Box::pin(Body {
            inner: Box::pin(response.bytes_stream()),
            decoder: OpenAiStream::new(),
            pending: std::collections::VecDeque::new(),
            cancel,
            finished: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_core::provider::{SystemBlock, ToolSpec, WireMessage};
    use std::sync::Arc;

    fn decode(body: &str) -> Vec<StreamEvent> {
        let mut s = OpenAiStream::new();
        let mut out = s.push(body.as_bytes()).unwrap();
        out.extend(s.finish().unwrap());
        out
    }

    #[test]
    fn text_deltas_become_one_block() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let events = decode(body);
        assert!(matches!(
            events[0],
            StreamEvent::BlockStart {
                kind: BlockKind::Text,
                ..
            }
        ));
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hello");
        assert!(matches!(
            events.last(),
            Some(StreamEvent::Done {
                stop: StopReason::EndTurn
            })
        ));
    }

    #[test]
    fn tool_call_fragments_accumulate_under_one_block() {
        let body = concat!(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":""}}]}}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"pa"}}]}}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.rs\"}"}}]}}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "\n\ndata: [DONE]\n\n"
        );
        let events = decode(body);

        let starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::BlockStart { .. }))
            .collect();
        assert_eq!(starts.len(), 1, "one call, one block");
        assert!(matches!(
            starts[0],
            StreamEvent::BlockStart {
                kind: BlockKind::ToolUse { .. },
                ..
            }
        ));

        let json: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolInputDelta { json, .. } => Some(json.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            serde_json::from_str::<Value>(&json).unwrap(),
            json!({"path": "a.rs"})
        );
        assert!(matches!(
            events.last(),
            Some(StreamEvent::Done {
                stop: StopReason::ToolUse
            })
        ));
    }

    #[test]
    fn a_stream_without_the_sentinel_is_truncated() {
        let mut s = OpenAiStream::new();
        let _ = s
            .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
            .unwrap();
        assert!(matches!(s.finish(), Err(ProviderError::Truncated)));
    }

    #[test]
    fn usage_is_surfaced_so_a_budget_can_be_enforced() {
        let body = concat!(
            r#"data: {"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":34}}"#,
            "\n\ndata: [DONE]\n\n"
        );
        let events = decode(body);
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::Usage(Usage {
                input_tokens: 120,
                output_tokens: 34,
                ..
            })
        )));
    }

    /// The seam. One user message carrying three tool results is correct for
    /// the Messages API and invalid here, where each result is its own message.

    #[test]
    fn the_system_prompt_becomes_the_first_message() {
        let mut req = ModelRequest::new("gpt-oss:120b");
        req.system = Arc::from(vec![SystemBlock {
            text: "you are axio".into(),
        }]);
        req.messages = vec![WireMessage {
            role: Role::User,
            content: vec![WireContent::Text { text: "hi".into() }],
        }];
        let body = build_body(&req);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn tools_are_wrapped_in_the_function_envelope() {
        let mut req = ModelRequest::new("gpt-oss:120b");
        req.tools = Arc::from(vec![ToolSpec {
            name: "read".into(),
            description: "Read a file.".into(),
            input_schema: json!({"type":"object"}),
        }]);
        let body = build_body(&req);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
        // `tool_choice` is documented as unsupported here, so it is never sent.
        assert!(body.get("tool_choice").is_none());
    }
}
