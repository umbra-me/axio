//! The whole vocabulary a surface sees.
//!
//! Every type here is `Serialize + Deserialize` even though nothing crosses a
//! process boundary yet. That is what makes `--json`, and later an out-of-process
//! client, additive rather than a redesign.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

mod outcome;

pub use outcome::{TurnOutcome, Usage};

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
        /// What the turn cost, matching the session record.
        ///
        /// A `--json` consumer could not see spend at all without parsing the
        /// session file, which is not a contract it has.
        cost_usd: f64,
        files_changed: Vec<PathBuf>,
    },
}

/// Something worth saying that is not fatal.
///
/// One type, so that config salvage, session load and budget validation all
/// reach the event stream through the same counter — a surface must never have
/// to invent a `seq`, because gap-free is a promise `--json` makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub level: NoticeLevel,
    pub message: String,
}

impl Notice {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            level: NoticeLevel::Info,
            message: message.into(),
        }
    }
    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            level: NoticeLevel::Warn,
            message: message.into(),
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: NoticeLevel::Error,
            message: message.into(),
        }
    }
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
    ///
    /// `signature` is the opaque token the provider mints alongside the block.
    /// Echoing the text without it is rejected, so it is part of the durable
    /// record even though no surface ever displays it.
    Reasoning {
        text: String,
        #[serde(default)]
        signature: String,
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

/// Struct variants throughout, deliberately: serde cannot serialise an
/// internally-tagged **newtype** variant wrapping a string — it fails at
/// runtime, not at compile time, with "cannot serialize tagged newtype variant".
/// A newtype `Text(String)` here would break every `--json` consumer on the
/// first streamed token while still compiling and passing a test that only ever
/// constructs the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "delta", rename_all = "snake_case")]
pub enum Delta {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    /// Argument fragments. Concatenate, parse once at block end. Surfaced so a
    /// TUI can render a command forming.
    ToolInputJson {
        json: String,
    },
    ToolOutput {
        stream: OutStream,
        text: String,
    },
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
    ///
    /// `raw` is what the shell is actually handed, and an approval surface must
    /// show it. The word split is lossy in exactly the direction that matters:
    /// `cat <<'EOF' > greet.py` lexes to a `cat` with some arguments, so the
    /// redirect target reads as an operand and the heredoc disappears — a
    /// reviewer sees something harmless and approves a write.
    Command {
        program: String,
        argv: Vec<String>,
        raw: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every arm has to say something, and none of it may be a struct literal.
    #[test]
    fn every_outcome_summarises_itself_in_prose() {
        let outcomes = [
            TurnOutcome::Completed,
            TurnOutcome::Refused {
                category: Some("cyber".into()),
                text: String::new(),
            },
            TurnOutcome::Refused {
                category: None,
                text: String::new(),
            },
            TurnOutcome::Interrupted,
            TurnOutcome::StepLimit { steps: 50 },
            TurnOutcome::BudgetExceeded {
                spent_usd: 2.5,
                limit_usd: 2.0,
            },
            TurnOutcome::Failed {
                message: "no credential".into(),
            },
        ];
        for outcome in outcomes {
            let said = outcome.summary();
            assert!(!said.is_empty(), "{outcome:?} says nothing");
            assert!(
                !said.contains('{') && !said.contains("::"),
                "{said} is a debug rendering, not a sentence"
            );
        }
        assert_eq!(
            TurnOutcome::StepLimit { steps: 50 }.summary(),
            "stopped at the step limit (50 steps)"
        );
    }

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

    /// Regression: an internally-tagged newtype variant wrapping a string fails
    /// to serialise at runtime while compiling perfectly happily. Every variant
    /// of every protocol enum must survive a round trip, and the only way to
    /// know is to actually do it.
    #[test]
    fn every_delta_variant_round_trips() {
        let deltas = [
            Delta::Text { text: "hi".into() },
            Delta::Reasoning {
                text: "thinking".into(),
            },
            Delta::ToolInputJson {
                json: "{\"a\":1}".into(),
            },
            Delta::ToolOutput {
                stream: OutStream::Stderr,
                text: "warn".into(),
            },
        ];
        for delta in deltas {
            let json = serde_json::to_value(&delta)
                .unwrap_or_else(|e| panic!("{delta:?} failed to serialise: {e}"));
            assert!(json.get("delta").is_some(), "{json} lost its tag");
            let back: Delta = serde_json::from_value(json).unwrap();
            assert_eq!(back, delta);
        }
    }

    /// The same hazard one level up: `EventKind` has a newtype variant too.
    /// `Usage` is a struct so it serialises as a map and is fine — but that is
    /// a property of `Usage`, not of the enum, so it is worth pinning.
    #[test]
    fn every_event_kind_variant_serialises() {
        let kinds = vec![
            EventKind::TurnStarted,
            EventKind::SessionStarted {
                protocol: PROTOCOL_VERSION,
                session: SessionId::nil(),
                model: "claude-opus-5".into(),
                cwd: PathBuf::from("/w"),
                effort: crate::provider::Effort::XHigh,
                resumed: false,
            },
            EventKind::ItemStarted {
                item: Item::new(ItemBody::AgentMessage { text: "a".into() }),
            },
            EventKind::ItemDiscarded {
                id: ItemId::nil(),
                reason: "retry".into(),
            },
            EventKind::ItemDelta {
                id: ItemId::nil(),
                delta: Delta::Text { text: "t".into() },
            },
            EventKind::ApprovalResolved {
                id: ApprovalId::nil(),
                decision: Decision::Allow,
            },
            EventKind::Compacted {
                stage: 1,
                tokens_before: 10,
                tokens_after: 5,
            },
            EventKind::Usage(Usage::default()),
            EventKind::Notice {
                level: NoticeLevel::Warn,
                message: "truncated".into(),
            },
            EventKind::TurnEnded {
                outcome: TurnOutcome::Completed,
                usage: Usage::default(),
                cost_usd: 0.0,
                files_changed: vec![],
            },
        ];
        for kind in kinds {
            let ev = Event {
                seq: 1,
                session: SessionId::nil(),
                turn: None,
                at_ms: 0,
                kind,
            };
            let json = serde_json::to_value(&ev)
                .unwrap_or_else(|e| panic!("{:?} failed to serialise: {e}", ev.kind));
            assert!(json.get("type").is_some());
            assert!(json.get("seq").is_some(), "flatten dropped the envelope");
        }
    }

    /// Regression. A provider reports usage more than once per message and
    /// each report is the running total, so summing them bills the input
    /// twice — silently, and only visibly on the invoice.
    #[test]
    fn cumulative_usage_reports_are_merged_not_summed() {
        let mut usage = Usage::default();
        // message_start: input counts known, output still 1.
        usage.merge_cumulative(&Usage {
            input_tokens: 1_200,
            output_tokens: 1,
            cache_read_input_tokens: 900,
            ..Default::default()
        });
        // message_delta: same input, final output.
        usage.merge_cumulative(&Usage {
            input_tokens: 1_200,
            output_tokens: 450,
            cache_read_input_tokens: 900,
            ..Default::default()
        });
        assert_eq!(usage.input_tokens, 1_200, "input was double-counted");
        assert_eq!(usage.output_tokens, 450);
        assert_eq!(usage.cache_read_input_tokens, 900);
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
