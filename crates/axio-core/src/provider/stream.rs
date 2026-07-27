//! What a provider's stream says, in terms the loop understands.

use super::*;

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

/// Every variant that carries provider-supplied text carries it as [`Redacted`].
///
/// A 401 or 403 body is the response most likely to quote back what was sent,
/// and an intermediary — a corporate proxy, a custom base URL — may echo the
/// `x-api-key` header verbatim. `Auth` and `Transport` therefore redact for the
/// same reason `Http` does; the invariant is that no provider body reaches an
/// error, a log or a session file in the clear, not that one variant does.
/// Collects `input_json_delta` fragments and parses them exactly once.
///
/// Parsing per fragment is the obvious mistake: the fragments are arbitrary
/// string slices of a JSON document, so all but the last are invalid on their
/// own. The parse happens at block end and nowhere else, and `parse_count` is
/// exposed so a test can prove it.
#[derive(Debug, Default)]
pub struct ToolInputAccumulator {
    fragments: std::collections::BTreeMap<u32, String>,
    parse_count: usize,
}

impl ToolInputAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, index: u32, fragment: &str) {
        self.fragments.entry(index).or_default().push_str(fragment);
    }

    /// Take the assembled arguments for a block. An empty accumulation is an
    /// empty object, which is what a no-argument tool call streams.
    pub fn finish(&mut self, index: u32) -> Result<serde_json::Value, ProviderError> {
        let raw = self.fragments.remove(&index).unwrap_or_default();
        self.parse_count += 1;
        if raw.trim().is_empty() {
            return Ok(serde_json::json!({}));
        }
        serde_json::from_str(&raw).map_err(|source| ProviderError::Decode {
            raw: Redacted::new(raw),
            source,
        })
    }

    pub fn parse_count(&self) -> usize {
        self.parse_count
    }
}
