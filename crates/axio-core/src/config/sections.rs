//! The shape of a configuration file.
//!
//! Every section with a non-false boolean default carries a hand-written
//! `impl Default`: `#[serde(default = "…")]` does not fire for a table that is
//! absent altogether, so a derived default silently turns those fields off.

use super::*;

/// The resolved configuration. Every field concrete.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub model: ModelSection,
    pub budget: BudgetSection,
    pub tools: ToolsSection,
    pub permissions: PermissionsSection,
    pub output: OutputSection,
    pub sandbox: SandboxSection,
}

impl Config {
    pub fn tool_limits(&self) -> ToolLimits {
        ToolLimits {
            max_output_bytes: self.tools.max_output_bytes,
            timeout: std::time::Duration::from_secs(self.tools.timeout_secs),
            max_file_bytes: self.tools.max_file_bytes,
        }
    }

    pub fn reasoning(&self) -> ReasoningDisplay {
        if self.output.show_reasoning {
            ReasoningDisplay::Summarized
        } else {
            ReasoningDisplay::Omitted
        }
    }
}

/// Kernel-enforced filesystem confinement. Off by default and Linux-only.
///
/// Off by default because it is a real restriction with real failure modes: a
/// toolchain that reads something outside the allow-list stops working, and a
/// sandbox that silently breaks the build is worse than none. On means a shell
/// command cannot reach `~/.ssh` or axio's own credential file whatever else
/// goes wrong.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxSection {
    pub enabled: bool,
    /// Extra paths a command may read, for whatever the toolchain needs that
    /// the defaults do not cover.
    pub read: Vec<String>,
    /// Extra paths a command may write.
    pub write: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSection {
    pub name: String,
    pub effort: Effort,
    pub max_tokens: u32,
    /// Which dialect to speak. Two impls, chosen by name — deliberately not a
    /// registry: a second provider is a second implementation, not an
    /// extension point, until something needs it to be.
    pub provider: String,
    /// Override the endpoint. Mostly for talking to a compatible host.
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetSection {
    /// Stops a turn once the spend so far exceeds this. `None` is no ceiling.
    pub max_usd_per_turn: Option<f64>,
    pub max_steps: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsSection {
    pub max_output_bytes: usize,
    pub timeout_secs: u64,
    pub max_file_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PermissionsSection {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

/// Hand-written `Default`, not derived.
///
/// This is the trap decision #22 exists for: `#[serde(default = "…")]` fires
/// when a *field* is absent from a table that is present, and never when the
/// whole table is absent. A derived `Default` then yields `false` for a bool
/// whose documented default is `true`, and the only symptom is a feature
/// quietly switching itself off for anyone who never wrote the section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputSection {
    /// Ask the provider for a readable summary of its reasoning.
    pub show_reasoning: bool,
    /// Print what the turn cost. Defaults **on**: an agent that spends money
    /// without saying so is a week-one complaint.
    pub show_cost: bool,
}
