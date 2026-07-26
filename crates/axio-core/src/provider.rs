//! The provider seam. One method, always streaming.

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
                    StreamEvent::Usage(u) => usage.add(&u),
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

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub model: String,
    /// Frozen for the session. Caching is a prefix match, so a system prompt
    /// rebuilt per turn silently costs the whole cached prefix, and nothing
    /// errors when it happens.
    pub system: Arc<[SystemBlock]>,
    /// Owned per-request projection. The transcript itself is never sent.
    pub messages: Vec<WireMessage>,
    /// Serialised so the bytes are identical run to run. Non-deterministic tool
    /// JSON destroys the cache prefix.
    pub tools: Arc<[ToolSpec]>,
    /// Caps thinking and response text together on this model.
    pub max_tokens: u32,
    /// The only depth knob. `temperature` / `top_p` / `top_k` are 400s on this
    /// model and have no field here, so no config layer can reintroduce them.
    pub effort: Effort,
    pub reasoning: ReasoningDisplay,
    pub cache: CachePlan,
}

impl ModelRequest {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: Arc::from(Vec::new()),
            messages: Vec::new(),
            tools: Arc::from(Vec::new()),
            max_tokens: 64_000,
            effort: Effort::default(),
            reasoning: ReasoningDisplay::default(),
            cache: CachePlan::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemBlock {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// Rendered with `preserve_order`, so key order is the order we built it in
    /// and therefore stable across runs.
    pub input_schema: serde_json::Value,
}

/// The wire projection of a transcript. Built by `wire_messages()`; never
/// stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireMessage {
    pub role: Role,
    pub content: Vec<WireContent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireContent {
    Text {
        text: String,
    },
    /// Echoed back verbatim. Never reconstructed, never edited.
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    /// The default: documented as best for coding and agentic work, and worth
    /// re-measuring on a real workload before changing.
    #[default]
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

/// Where cache breakpoints go. The budget is four; we spend at most three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePlan {
    /// Breakpoint on the last tool spec.
    pub tools: bool,
    /// Breakpoint on the last system block.
    pub system: bool,
    /// Rolling breakpoint on the last content block of this message index.
    /// Re-placed every ~15 blocks, because the lookback window is 20 and a
    /// tool-heavy turn silently stops finding the previous entry.
    pub message: Option<usize>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub context_window: u64,
    pub max_output_tokens: u32,
    /// USD per million tokens.
    pub input_price: f64,
    pub output_price: f64,
    pub cache_read_price: f64,
    pub cache_write_price: f64,
}

impl ModelInfo {
    pub fn cost_usd(&self, usage: &Usage) -> f64 {
        let m = 1_000_000.0;
        (usage.input_tokens as f64 * self.input_price
            + usage.output_tokens as f64 * self.output_price
            + usage.cache_read_input_tokens as f64 * self.cache_read_price
            + usage.cache_creation_input_tokens as f64 * self.cache_write_price)
            / m
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    /// Arrives on the success path with HTTP 200. Never an error.
    Refusal {
        category: Option<String>,
    },
    /// A server-side tool loop paused; the caller re-sends to resume.
    PauseTurn,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    MessageStart {
        id: String,
    },
    BlockStart {
        index: u32,
        kind: BlockKind,
    },
    TextDelta {
        index: u32,
        text: String,
    },
    ReasoningDelta {
        index: u32,
        text: String,
    },
    /// Tool arguments arrive as JSON string fragments; concatenate and parse
    /// exactly once at `BlockEnd`.
    ToolInputDelta {
        index: u32,
        json: String,
    },
    /// The opaque signature that must be echoed back with a thinking block.
    ReasoningSignature {
        index: u32,
        signature: String,
    },
    BlockEnd {
        index: u32,
    },
    Usage(Usage),
    Done {
        stop: StopReason,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("authentication failed: {0}")]
    Auth(String),
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
    Transport(String),
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
