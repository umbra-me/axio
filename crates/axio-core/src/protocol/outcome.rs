//! How a turn ended, and what it used.
//!
//! Split out of `protocol` when that file passed the width limit. The two
//! belong together: both are what a surface reports *after* the work, and
//! neither is part of the streaming vocabulary above them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    /// A refusal arrives as a normal HTTP 200 with a category. It is an outcome,
    /// not an error, and must not be retried.
    Refused {
        category: Option<String>,
        text: String,
    },
    Interrupted,
    StepLimit {
        steps: u32,
    },
    BudgetExceeded {
        spent_usd: f64,
        limit_usd: f64,
    },
    Failed {
        message: String,
    },
}

impl TurnOutcome {
    /// How a turn ended, in a sentence.
    ///
    /// Here rather than in a surface for the reason every other piece of
    /// user-facing wording is: two surfaces describing the same outcome
    /// differently is a bug nobody files. It also replaces the `{:?}` that had
    /// started appearing in output people actually read — `StepLimit { steps:
    /// 50 }` is a struct literal, not an explanation.
    pub fn summary(&self) -> String {
        match self {
            Self::Completed => "completed".to_owned(),
            Self::Refused { category, .. } => match category {
                Some(category) => format!("refused ({category})"),
                None => "refused".to_owned(),
            },
            Self::Interrupted => "interrupted".to_owned(),
            Self::StepLimit { steps } => {
                format!("stopped at the step limit ({steps} steps)")
            }
            Self::BudgetExceeded {
                spent_usd,
                limit_usd,
            } => format!("stopped at the spend cap (${spent_usd:.2} of ${limit_usd:.2})"),
            Self::Failed { message } => format!("failed: {message}"),
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Completed => 0,
            Self::Failed { .. } => 1,
            Self::StepLimit { .. } | Self::BudgetExceeded { .. } => 3,
            Self::Refused { .. } => 4,
            Self::Interrupted => 130,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl Usage {
    /// Field-wise accumulation across the steps of one turn.
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }

    /// Merge a report that carries a running total for one message.
    ///
    /// A provider sends usage more than once per message — once at the start
    /// with the input counts and once at the end with the output count — and
    /// each report is the total so far, not a delta. Summing them bills the
    /// input twice. Taking the field-wise maximum is correct for a cumulative
    /// report and harmless for a single one.
    pub fn merge_cumulative(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .max(other.cache_creation_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .max(other.cache_read_input_tokens);
    }

    /// Billable input, counting a cache read at its own rate elsewhere.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
    }
}
