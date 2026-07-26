//! A provider speaking the OpenAI chat-completions dialect.
//!
//! This exists to answer a question the design could not answer by inspection:
//! is the `Provider` trait actually provider-shaped, or is it the Messages API
//! wearing a trait? The answer turned out to be mostly the former, with one
//! real seam — see `wire_to_openai`, where a single user message carrying N
//! tool results has to become N separate messages. That conversion belongs in
//! the provider, which is the point of having the seam at all.
//!
//! Two things this dialect does not have, and what happens to them:
//!
//! * **Effort.** There is no equivalent, so it is dropped. A request that would
//!   have asked for deep reasoning simply does not.
//! * **Reasoning blocks.** No equivalent either, so they are not replayed. That
//!   is safe rather than lossy: the transcript keeps them, and the projection
//!   already drops blocks minted by a different model.

use std::pin::Pin;

use axio_core::protocol::Usage;
use axio_core::provider::{
    BlockKind, BoxStream, ModelInfo, ModelRequest, Provider, ProviderError, Role, StopReason,
    StreamEvent, WireContent,
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

/// Build the request body in the chat-completions dialect.
pub fn build_body(req: &ModelRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    // The system prompt is a message here rather than a top-level field.
    if !req.system.is_empty() {
        let text = req
            .system
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        messages.push(json!({ "role": "system", "content": text }));
    }
    messages.extend(wire_to_openai(&req.messages));

    let mut body = Map::new();
    body.insert("model".into(), json!(req.model));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("stream".into(), json!(true));
    // Without this the stream carries no usage at all, and a turn that cannot
    // report what it cost cannot enforce a budget either.
    body.insert("stream_options".into(), json!({ "include_usage": true }));
    body.insert("max_tokens".into(), json!(req.max_tokens));

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }

    Value::Object(body)
}

/// Convert the wire projection into this dialect's message list.
///
/// This is the seam the trait was uncertain about. One user message carrying
/// three `tool_result`s is correct for the Messages API and invalid here, where
/// each result is its own message with `role: "tool"`. Splitting is mechanical,
/// which is the useful finding: the shape difference is real but local to the
/// provider, and nothing above it had to change.
fn wire_to_openai(messages: &[axio_core::provider::WireMessage]) -> Vec<Value> {
    let mut out = Vec::new();

    for message in messages {
        let mut text = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut results: Vec<Value> = Vec::new();

        for content in &message.content {
            match content {
                WireContent::Text { text: t } => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
                // No equivalent, and inventing one would be worse than
                // omitting it. The transcript keeps the block either way.
                WireContent::Thinking { .. } => {}
                WireContent::ToolUse { id, name, input } => tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        // Arguments are a JSON *string* here, not an object.
                        "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                    }
                })),
                WireContent::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => results.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                })),
            }
        }

        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        if !text.is_empty() || !tool_calls.is_empty() {
            let mut m = Map::new();
            m.insert("role".into(), json!(role));
            m.insert("content".into(), json!(text));
            if !tool_calls.is_empty() {
                m.insert("tool_calls".into(), Value::Array(tool_calls));
            }
            out.push(Value::Object(m));
        }
        // Each result becomes its own message, in order.
        out.extend(results);
    }
    out
}

/// Turns chat-completion chunks into protocol events.
#[derive(Debug, Default)]
pub struct OpenAiStream {
    decoder: SseDecoder,
    /// Blocks already announced, so a `BlockStart` is emitted exactly once.
    started: std::collections::BTreeSet<u32>,
    text_index: Option<u32>,
    saw_done: bool,
    next_index: u32,
}

impl OpenAiStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        let frames = self.decoder.push(chunk);
        let mut out = Vec::new();
        for frame in frames {
            self.map(&frame.data, &mut out)?;
        }
        Ok(out)
    }

    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        let frames = self.decoder.finish();
        let mut out = Vec::new();
        for frame in frames {
            self.map(&frame.data, &mut out)?;
        }
        // Close any block still open, so the loop sees a well-formed message.
        for index in std::mem::take(&mut self.started) {
            out.push(StreamEvent::BlockEnd { index });
        }
        if !self.saw_done {
            return Err(ProviderError::Truncated);
        }
        Ok(out)
    }

    fn map(&mut self, data: &str, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        let data = data.trim();
        if data.is_empty() {
            return Ok(());
        }
        // This dialect ends with a sentinel rather than an event type.
        if data == "[DONE]" {
            self.saw_done = true;
            return Ok(());
        }

        let v: Value = serde_json::from_str(data).map_err(|source| ProviderError::Decode {
            raw: Redacted::new(data.to_owned()),
            source,
        })?;

        if let Some(error) = v.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(ProviderError::Transport(Redacted::new(message.to_owned())));
        }

        if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
            out.push(StreamEvent::Usage(parse_usage(usage)));
        }

        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
            return Ok(());
        };

        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(Value::as_str)
                && !text.is_empty()
            {
                let index = match self.text_index {
                    Some(i) => i,
                    None => {
                        let i = self.take_index();
                        self.text_index = Some(i);
                        self.started.insert(i);
                        out.push(StreamEvent::BlockStart {
                            index: i,
                            kind: BlockKind::Text,
                        });
                        i
                    }
                };
                out.push(StreamEvent::TextDelta {
                    index,
                    text: text.to_owned(),
                });
            }

            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    // The dialect's own per-call index, offset so it cannot
                    // collide with the text block.
                    let raw = call.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let index = raw + 1_000;

                    if self.started.insert(index) {
                        out.push(StreamEvent::BlockStart {
                            index,
                            kind: BlockKind::ToolUse {
                                id: call
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                name: call
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                            },
                        });
                    }
                    if let Some(args) = call
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(Value::as_str)
                        && !args.is_empty()
                    {
                        out.push(StreamEvent::ToolInputDelta {
                            index,
                            json: args.to_owned(),
                        });
                    }
                }
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            for index in std::mem::take(&mut self.started) {
                out.push(StreamEvent::BlockEnd { index });
            }
            self.text_index = None;
            out.push(StreamEvent::Done {
                stop: match reason {
                    "tool_calls" | "function_call" => StopReason::ToolUse,
                    "length" => StopReason::MaxTokens,
                    "content_filter" => StopReason::Refusal { category: None },
                    _ => StopReason::EndTurn,
                },
            });
        }
        Ok(())
    }

    fn take_index(&mut self) -> u32 {
        let i = self.next_index;
        self.next_index += 1;
        i
    }
}

fn parse_usage(v: &Value) -> Usage {
    let get = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input_tokens: get("prompt_tokens"),
        output_tokens: get("completion_tokens"),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
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
    fn a_batch_of_tool_results_becomes_one_message_each() {
        let messages = vec![
            WireMessage {
                role: Role::Assistant,
                content: vec![
                    WireContent::ToolUse {
                        id: "call_1".into(),
                        name: "read".into(),
                        input: json!({"path": "a.rs"}),
                    },
                    WireContent::ToolUse {
                        id: "call_2".into(),
                        name: "read".into(),
                        input: json!({"path": "b.rs"}),
                    },
                ],
            },
            WireMessage {
                role: Role::User,
                content: vec![
                    WireContent::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "a".into(),
                        is_error: false,
                    },
                    WireContent::ToolResult {
                        tool_use_id: "call_2".into(),
                        content: "b".into(),
                        is_error: false,
                    },
                ],
            },
        ];
        let out = wire_to_openai(&messages);

        assert_eq!(out.len(), 3, "one assistant message plus two tool messages");
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[0]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(out[1]["role"], "tool");
        assert_eq!(out[1]["tool_call_id"], "call_1");
        assert_eq!(out[2]["tool_call_id"], "call_2");
    }

    #[test]
    fn tool_arguments_are_a_string_not_an_object() {
        let messages = vec![WireMessage {
            role: Role::Assistant,
            content: vec![WireContent::ToolUse {
                id: "call_1".into(),
                name: "read".into(),
                input: json!({"path": "a.rs"}),
            }],
        }];
        let out = wire_to_openai(&messages);
        let args = out[0]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .expect("arguments must be a JSON string in this dialect");
        assert_eq!(
            serde_json::from_str::<Value>(args).unwrap(),
            json!({"path": "a.rs"})
        );
    }

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

    #[test]
    fn reasoning_blocks_are_dropped_rather_than_mistranslated() {
        let messages = vec![WireMessage {
            role: Role::Assistant,
            content: vec![
                WireContent::Thinking {
                    thinking: "considering".into(),
                    signature: "sig".into(),
                },
                WireContent::Text {
                    text: "the answer".into(),
                },
            ],
        }];
        let out = wire_to_openai(&messages);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["content"], "the answer");
        assert!(out[0].get("thinking").is_none());
    }
}
