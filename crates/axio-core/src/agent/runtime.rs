//! What a run was configured to do, resolved once and then fixed.
//!
//! Derived from the layered configuration at startup so the loop never reads a
//! file, and a turn cannot change its own limits halfway through.

use super::*;

/// Resolved once, at construction, and never re-read.
///
/// The predecessor project read config inside the loop on every turn and had to
/// carry a written rule forbidding other reads. A rule in a document is not an
/// enforcement mechanism; a value that can only be produced by the resolver is.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub model: String,
    pub effort: Effort,
    pub reasoning: ReasoningDisplay,
    pub max_tokens: u32,
    pub max_steps: u32,
    /// Successive delays between retries. Running out of entries makes the
    /// error fatal, so the length is also the retry budget.
    pub backoff: Vec<Duration>,
    pub max_usd_per_turn: Option<f64>,
    /// The cap applied to every tool result, at one choke point in the loop.
    pub max_output_bytes: usize,
    /// Where output too large to send is kept so the model can read it back.
    pub spill_dir: Option<std::path::PathBuf>,
    pub tool_limits: ToolLimits,
}

impl RuntimeConfig {
    /// The only way to build one from configuration.
    ///
    /// Everything else in the loop reads `self.cfg`, so funnelling construction
    /// through here is what makes "config is resolved once" true by
    /// construction rather than by everyone remembering.
    pub fn from_resolved(resolved: &crate::config::Resolved) -> Self {
        let c = resolved.config();
        Self {
            model: c.model.name.clone(),
            effort: c.model.effort,
            reasoning: c.reasoning(),
            max_tokens: c.model.max_tokens,
            max_steps: c.budget.max_steps,
            max_usd_per_turn: c.budget.max_usd_per_turn,
            tool_limits: c.tool_limits(),
            // The loop's own choke point reads this field, so leaving it at the
            // default made `[tools] max_output_bytes` a setting that `--explain`
            // reported and nothing obeyed.
            max_output_bytes: c.tools.max_output_bytes,
            ..Self::default()
        }
    }

    /// `--resume`. The model comes from the session header, because a
    /// transcript's reasoning blocks are only replayable under the model that
    /// minted them.
    pub fn adopt_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            model: "claude-opus-5".to_owned(),
            effort: Effort::default(),
            reasoning: ReasoningDisplay::default(),
            max_tokens: 64_000,
            max_steps: 50,
            backoff: vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(5),
            ],
            max_usd_per_turn: None,
            max_output_bytes: 64 * 1024,
            spill_dir: None,
            tool_limits: ToolLimits::default(),
        }
    }
}
