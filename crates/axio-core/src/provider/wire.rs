//! The request as a provider sees it, and the vocabulary it answers in.
//!
//! Dependency-free and serde-derived from the first commit, which is what makes
//! `--json` a renderer rather than a second loop.

use super::*;

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
