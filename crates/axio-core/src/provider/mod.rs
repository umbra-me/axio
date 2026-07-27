//! The provider seam. One method, always streaming.

mod stream;
mod wire;

pub use stream::{BlockKind, StopReason, StreamEvent, ToolInputAccumulator};
pub use wire::{
    CachePlan, ModelInfo, ModelRequest, Role, SystemBlock, ToolSpec, WireContent, WireMessage,
};

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::protocol::Usage;
use crate::redact::Redacted;

pub type BoxStream<'a, T> = Pin<Box<dyn futures_core::Stream<Item = T> + Send + 'a>>;

#[async_trait::async_trait]
pub trait Provider: Send + Sync + 'static {
    fn id(&self) -> &str;
    fn model_info(&self, model: &str) -> ModelInfo;

    /// Always streams. There is no `stream: bool` knob anywhere in axio.
    /// Non-streaming is an internal detail of [`complete`].
    async fn stream(
        &self,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError>;
}

/// The single one-shot helper. Lives here so no caller is tempted to construct
/// a non-streaming request.
pub async fn complete(
    provider: &dyn Provider,
    req: ModelRequest,
    cancel: CancellationToken,
) -> Result<(String, Usage), ProviderError> {
    use futures_core::Stream;

    let mut stream = provider.stream(req, cancel).await?;
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut done = false;

    std::future::poll_fn(|cx| {
        loop {
            match Pin::new(&mut stream).poll_next(cx) {
                std::task::Poll::Ready(Some(Ok(ev))) => match ev {
                    StreamEvent::TextDelta { text: t, .. } => text.push_str(&t),
                    // Cumulative, not incremental: message_start and
                    // message_delta each report the running total, so adding
                    // both counts the input tokens twice.
                    StreamEvent::Usage(u) => usage.merge_cumulative(&u),
                    StreamEvent::Done { .. } => done = true,
                    _ => {}
                },
                std::task::Poll::Ready(Some(Err(e))) => {
                    return std::task::Poll::Ready(Err(e));
                }
                std::task::Poll::Ready(None) => return std::task::Poll::Ready(Ok(())),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    })
    .await?;

    if !done {
        return Err(ProviderError::Truncated);
    }
    Ok((text, usage))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    /// The default: documented as best for coding and agentic work, and worth
    /// re-measuring on a real workload before changing.
    ///
    /// The rename is load-bearing: `rename_all = "snake_case"` spells this
    /// `x_high`, which is neither what `as_wire` sends nor what a user would
    /// write in a config file — and it leaked into the `--json` stream while
    /// the request body said `xhigh`. The aliases keep an already-written
    /// config or session file loadable.
    #[default]
    #[serde(rename = "xhigh", alias = "x_high", alias = "x-high")]
    XHigh,
    Max,
}

impl Effort {
    pub fn as_wire(&self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// Whether to ask for a readable summary of the model's reasoning.
///
/// This changes a request field only. Reasoning blocks are persisted and echoed
/// back either way, because they are wire state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningDisplay {
    /// The default on this model: blocks arrive with empty text.
    #[default]
    Omitted,
    Summarized,
}

impl Default for CachePlan {
    fn default() -> Self {
        Self {
            tools: true,
            system: true,
            message: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("authentication failed: {0}")]
    Auth(Redacted),
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("overloaded")]
    Overloaded,
    #[error("context window exceeded")]
    ContextOverflow,
    #[error("http {status}: {body}")]
    Http { status: u16, body: Redacted },
    #[error("stream ended mid-message")]
    Truncated,
    #[error("transport: {0}")]
    Transport(Redacted),
    /// A mistake in the configuration, not a bad moment on the network.
    ///
    /// Separate from `Transport` because retrying it is pure waste: a base URL
    /// that will not parse produces the same failure on every attempt, and the
    /// backoff schedule turned an instant, fixable error into eight silent
    /// seconds followed by the same unhelpful message.
    #[error("configuration: {0}")]
    Configuration(String),
    /// `raw` is the verbatim SSE span. Keeping it is the difference between a
    /// debuggable bug report and a shrug.
    #[error("decode failed: {source}")]
    Decode {
        raw: Redacted,
        #[source]
        source: serde_json::Error,
    },
    #[error("cancelled")]
    Cancelled,
}

impl ProviderError {
    /// The loop asks exactly one question of a provider error.
    pub fn retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. } | Self::Overloaded | Self::Truncated | Self::Transport(_) => {
                true
            }
            Self::Http { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Everything a `Display` chain would hide.
    ///
    /// `reqwest`'s `Display` prints only its outermost layer, so the cause —
    /// `relative URL without a base`, `connection refused`, `dns error`,
    /// `certificate verify failed` — is thrown away and the user is told
    /// "builder error", which names neither the problem nor the setting.
    pub fn full_chain(e: &(dyn std::error::Error + 'static)) -> String {
        let mut parts = vec![e.to_string()];
        let mut source = e.source();
        while let Some(cause) = source {
            let text = cause.to_string();
            if !parts.iter().any(|p| p == &text) {
                parts.push(text);
            }
            source = cause.source();
        }
        parts.join(": ")
    }

    pub fn backoff_hint(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression. `rename_all = "snake_case"` spelled this `x_high`, so the
    /// `--json` stream said `x_high` while the request body said `xhigh`, and a
    /// config file written with the wire spelling would not load.
    #[test]
    fn effort_serialises_the_way_it_goes_on_the_wire() {
        for effort in [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
        ] {
            let json = serde_json::to_string(&effort).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", effort.as_wire()),
                "serde and as_wire disagree for {effort:?}"
            );
            assert_eq!(serde_json::from_str::<Effort>(&json).unwrap(), effort);
        }
        // The old spelling still loads, so a session or config already written
        // with it is not orphaned.
        assert_eq!(
            serde_json::from_str::<Effort>("\"x_high\"").unwrap(),
            Effort::XHigh
        );
    }

    #[test]
    fn effort_defaults_to_xhigh() {
        assert_eq!(Effort::default(), Effort::XHigh);
        assert_eq!(Effort::default().as_wire(), "xhigh");
    }

    #[test]
    fn retryable_classification() {
        assert!(ProviderError::RateLimited { retry_after: None }.retryable());
        assert!(ProviderError::Overloaded.retryable());
        assert!(ProviderError::Truncated.retryable());
        assert!(
            ProviderError::Http {
                status: 503,
                body: "".into()
            }
            .retryable()
        );
        assert!(
            !ProviderError::Http {
                status: 400,
                body: "".into()
            }
            .retryable()
        );
        assert!(!ProviderError::ContextOverflow.retryable());
        assert!(!ProviderError::Auth("no key".into()).retryable());
        assert!(!ProviderError::Cancelled.retryable());
    }

    #[test]
    fn backoff_hint_only_from_rate_limit() {
        let e = ProviderError::RateLimited {
            retry_after: Some(Duration::from_secs(3)),
        };
        assert_eq!(e.backoff_hint(), Some(Duration::from_secs(3)));
        assert_eq!(ProviderError::Overloaded.backoff_hint(), None);
    }

    #[test]
    fn cost_is_computed_per_million() {
        let info = ModelInfo {
            context_window: 1_000_000,
            max_output_tokens: 64_000,
            input_price: 5.0,
            output_price: 25.0,
            cache_read_price: 0.5,
            cache_write_price: 6.25,
        };
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        assert!((info.cost_usd(&usage) - 30.0).abs() < f64::EPSILON);
    }
}
