//! The whole vocabulary a surface sees.
//!
//! Every type here is `Serialize + Deserialize` even though nothing crosses a
//! process boundary yet. That is what makes `--json`, and later an out-of-process
//! client, additive rather than a redesign.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type SessionId = ulid::Ulid;
pub type TurnId = ulid::Ulid;
pub type ItemId = ulid::Ulid;
pub type ApprovalId = ulid::Ulid;

/// Bumped only on a breaking change to this module. Emitted in `SessionStarted`
/// so a consumer can refuse a stream it does not understand.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic and gap-free per session. Nothing replays in v1; a renderer
    /// that sees a gap knows it dropped events.
    pub seq: u64,
    pub session: SessionId,
    pub turn: Option<TurnId>,
    pub at_ms: u64,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// Canonical items carrying their own status, plus separate deltas for
/// streaming. Deliberately not paired Begin/End variants per tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    SessionStarted {
        protocol: u32,
        session: SessionId,
        model: String,
        cwd: PathBuf,
        effort: crate::provider::Effort,
        resumed: bool,
    },
    TurnStarted,
    /// A durable transcript item appeared. Renderers append.
    ItemStarted {
        item: Item,
    },
    /// Same id, new status or content. Renderers replace by id.
    ItemUpdated {
        item: Item,
    },
    ItemCompleted {
        item: Item,
    },
    /// The in-flight item is being abandoned before a retry. Renderers drop by
    /// id. Without this, a retry double-prints text the user has already seen.
    ItemDiscarded {
        id: ItemId,
        reason: String,
    },
    /// Presentation only. A renderer that drops every delta still produces a
    /// correct transcript from the item events alone.
    ItemDelta {
        id: ItemId,
        delta: Delta,
    },

    /// Observability only. The decision travels back through `Approver`.
    ApprovalRequested {
        id: ApprovalId,
        request: ApprovalRequest,
    },
    ApprovalResolved {
        id: ApprovalId,
        decision: Decision,
    },

    Compacted {
        stage: u8,
        tokens_before: u64,
        tokens_after: u64,
    },
    Usage(Usage),
    /// Non-fatal, and never silently swallowed.
    Notice {
        level: NoticeLevel,
        message: String,
    },
    TurnEnded {
        outcome: TurnOutcome,
        usage: Usage,
        files_changed: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    #[serde(flatten)]
    pub body: ItemBody,
}

impl Item {
    pub fn new(body: ItemBody) -> Self {
        Self {
            id: ItemId::generate(),
            body,
        }
    }
}

// `ToolCall` is much larger than the message variants, and clippy is right that
// this costs memory per item. Boxing is still the wrong trade here: this is a
// protocol type constructed at every call site and matched on in every renderer,
// and `ToolCall` is the variant a real session is mostly made of — so the
// indirection would be paid on the common path to save space on the rare one.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum ItemBody {
    UserMessage {
        text: String,
    },
    AgentMessage {
        text: String,
    },
    /// On this model the raw chain of thought is never returned: `text` is a
    /// summary when `display: "summarized"` is requested and empty otherwise.
    /// The block is still wire state and must be echoed back verbatim, which is
    /// why it is persisted regardless of whether a surface renders it.
    Reasoning {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        input: serde_json::Value,
        /// Policy match key, e.g. `bash:git`.
        subject: String,
        /// From `plan()`; a diff or an argv.
        preview: Option<Preview>,
        #[serde(flatten)]
        status: ToolStatus,
    },
    /// Written when a turn is cancelled, so the model learns on the next request
    /// that its work was cut short.
    Interrupted {
        after_steps: u32,
    },
    /// Deterministic elision removed context here. Present so a replayed
    /// transcript shows the hole rather than silently lying.
    ContextElision {
        dropped_items: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    AwaitingApproval,
    Running,
    Ok {
        output: String,
        truncated: bool,
        spill: Option<PathBuf>,
        ms: u64,
    },
    Failed {
        message: String,
    },
    /// The model receives this as a tool_result and the turn continues.
    Denied {
        message: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "delta", rename_all = "snake_case")]
pub enum Delta {
    Text(String),
    Reasoning(String),
    /// Argument fragments. Concatenate, parse once at block end. Surfaced so a
    /// TUI can render a command forming.
    ToolInputJson(String),
    ToolOutput {
        stream: OutStream,
        text: String,
    },
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Preview {
    Diff {
        path: PathBuf,
        unified: String,
        added: u32,
        removed: u32,
    },
    /// Structured, not a shell string — the permission engine matches on
    /// `program`, so a compound command cannot pass as a simple one.
    Command {
        program: String,
        argv: Vec<String>,
        cwd: PathBuf,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub call_id: String,
    pub tool: String,
    /// What an `AllowSession` grant is remembered against. Shown verbatim.
    pub subject: String,
    pub effects: crate::tool::Effects,
    pub preview: Option<Preview>,
    /// Why policy could not decide alone. The engine owns the wording so two
    /// surfaces cannot drift.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Allow,
    /// Memory-only, this process. Nothing is written to config by an approval.
    AllowSession,
    /// `feedback` becomes the tool_result the model sees, so "no, use the
    /// existing helper" is a steering channel rather than a dead end.
    Deny {
        feedback: Option<String>,
    },
}
// There is deliberately no `Abort` variant. Two different "no"s with different
// consequences is a UX trap; cancelling the turn already has its own path.

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

    /// Billable input, counting a cache read at its own rate elsewhere.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_turn_exits_130() {
        assert_eq!(TurnOutcome::Interrupted.exit_code(), 130);
        assert_eq!(TurnOutcome::Completed.exit_code(), 0);
        assert_eq!(
            TurnOutcome::Refused {
                category: Some("cyber".into()),
                text: String::new()
            }
            .exit_code(),
            4
        );
    }

    #[test]
    fn event_kind_is_externally_readable() {
        let ev = Event {
            seq: 1,
            session: SessionId::nil(),
            turn: None,
            at_ms: 0,
            kind: EventKind::TurnStarted,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "turn_started");
        assert_eq!(json["seq"], 1);
    }

    #[test]
    fn tool_call_status_flattens_without_colliding() {
        let item = Item::new(ItemBody::ToolCall {
            call_id: "toolu_1".into(),
            name: "read".into(),
            input: serde_json::json!({"path": "a.rs"}),
            subject: "read:a.rs".into(),
            preview: None,
            status: ToolStatus::Ok {
                output: "hi".into(),
                truncated: false,
                spill: None,
                ms: 3,
            },
        });
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["item"], "tool_call");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["output"], "hi");
        // Round-trips: flatten + two internal tags must not eat each other.
        let back: Item = serde_json::from_value(json).unwrap();
        assert!(matches!(back.body, ItemBody::ToolCall { .. }));
    }

    #[test]
    fn usage_accumulates() {
        let mut a = Usage {
            input_tokens: 10,
            output_tokens: 2,
            ..Default::default()
        };
        a.add(&Usage {
            input_tokens: 5,
            cache_read_input_tokens: 7,
            ..Default::default()
        });
        assert_eq!(a.input_tokens, 15);
        assert_eq!(a.cache_read_input_tokens, 7);
        assert_eq!(a.total_tokens(), 24);
    }
}
