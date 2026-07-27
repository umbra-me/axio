//! The Anthropic Messages transport.
//!
//! Three things live here: the request builder (whose byte-stability is what
//! keeps the prompt cache alive), the stream state machine that turns SSE frames
//! into [`StreamEvent`]s, and error classification. Everything the loop needs to
//! know about a failure is answered by [`ProviderError::retryable`].

mod errors;
mod request;
mod stream;

pub use errors::classify;
pub use request::{build_body, rolling_cache_plan};
pub use stream::AnthropicStream;

use std::time::Duration;

use axio_core::protocol::Usage;
use axio_core::provider::{
    BlockKind, CachePlan, ModelRequest, ProviderError, ReasoningDisplay, Role, StopReason,
    StreamEvent, WireContent,
};
use axio_core::redact::Redacted;
use serde_json::{Map, Value, json};

use crate::sse::{SseDecoder, SseFrame};

pub use axio_core::provider::ToolInputAccumulator;

pub const DEFAULT_MODEL: &str = "claude-opus-5";

/// The prices and limits this provider assumes, without needing an instance —
/// so `--doctor` can report what a run would actually use without touching a
/// credential or building an HTTP client.
pub fn model_info(_model: &str) -> axio_core::provider::ModelInfo {
    // One hardcoded model, so the price table cannot drift far.
    axio_core::provider::ModelInfo {
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.5,
        cache_write_price: 6.25,
    }
}
pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2023-06-01";
