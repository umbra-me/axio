//! Every shape that crosses into the webview.
//!
//! One module, deliberately, because this is the boundary and a boundary
//! scattered across a codebase is one nobody can audit. Everything here is
//! `Serialize` with `rename_all = "camelCase"`, so the TypeScript side reads
//! naturally without either side translating.
//!
//! These types are **projections**, never the real thing. A `SessionView` is
//! what a list row needs; the session itself stays in `axio-supervisor` where
//! it can be cancelled and closed. That is the whole architecture in one rule:
//! the webview is shown state, and never handed the means to hold it.
//!
//! The next step for this file is generating its TypeScript rather than
//! mirroring it. The prior art here maintains both sides by hand and warns, in
//! four separate documents, that "changing one side without the other is a
//! silent break" — and it has already drifted: one field typed as a union in
//! TypeScript is an unvalidated `String` in Rust, and the value stored keeps
//! whatever casing the caller sent. A warning repeated four times is a job for
//! a build step.

use serde::{Deserialize, Serialize};

/// A repository under supervision.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub root: String,
    /// Sessions that have not been closed. What a rail badge counts.
    pub open_sessions: usize,
    pub total_sessions: usize,
}

/// What a session looks like in a list.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SessionView {
    pub id: String,
    /// Eight characters. Long enough to be unambiguous in practice, short
    /// enough to read — the same prefix the CLI prints and accepts.
    pub short_id: String,
    pub project_id: String,
    pub project_name: String,
    /// The first prompt. `None` for a session started without one.
    pub label: Option<String>,
    pub branch: Option<String>,
    pub workspace: String,
    pub isolation: Isolation,
    pub status: SessionStatus,
    /// `number`, not `bigint`. ts-rs maps `u64` to `bigint` by default, which
    /// would be right for a boundary that preserved 64-bit integers — and this
    /// one does not: Tauri's IPC is JSON, so what actually arrives is a JS
    /// number. Declaring `bigint` would be a type that never matches the value.
    /// Both quantities here are safe below 2^53: a millisecond timestamp until
    /// the year 287396, and a byte cursor until nine petabytes through one
    /// terminal.
    #[ts(type = "number")]
    pub started_ms: u64,
}

/// Where a session does its work.
///
/// An enum rather than a bool because a third answer is plausible — a container,
/// a remote host — and a bool would have to be replaced rather than extended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum Isolation {
    /// Its own git worktree on its own branch. The default.
    Worktree,
    /// The repository as it sits.
    Direct,
}

/// What a session is doing.
///
/// Deliberately not a `String`. The prior art types this as a union on the
/// TypeScript side and as `String` in Rust, so the two vocabularies drifted:
/// one end knows about states the other never produces, and the value that
/// reaches the interface can violate the type the interface declares for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum SessionStatus {
    /// Live and between turns.
    Idle,
    /// A turn is in flight.
    Running,
    /// Not live. Its worktree may still hold work.
    Closed,
}

/// A question a session is waiting on.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ApprovalView {
    pub id: String,
    pub session_id: String,
    pub short_session_id: String,
    pub project_id: String,
    /// What an "allow for this session" grant would be remembered against.
    /// Shown verbatim, because it is the thing being agreed to.
    pub subject: String,
    pub tool: String,
    /// Why policy could not decide alone. The engine owns this wording so two
    /// surfaces cannot describe the same refusal differently.
    pub reason: String,
    pub preview: Option<PreviewView>,
    /// `number`, not `bigint`. ts-rs maps `u64` to `bigint` by default, which
    /// would be right for a boundary that preserved 64-bit integers — and this
    /// one does not: Tauri's IPC is JSON, so what actually arrives is a JS
    /// number. Declaring `bigint` would be a type that never matches the value.
    /// Both quantities here are safe below 2^53: a millisecond timestamp until
    /// the year 287396, and a byte cursor until nine petabytes through one
    /// terminal.
    #[ts(type = "number")]
    pub at_ms: u64,
}

/// What an approval is about, in the shape a reviewer needs to see it.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum PreviewView {
    Diff {
        path: String,
        unified: String,
        added: u32,
        removed: u32,
    },
    /// `raw` is what the shell actually receives and is the only honest thing
    /// to show. A word-split reads as a simpler command than the one that runs:
    /// a heredoc disappears and a redirect looks like an operand, so a reviewer
    /// approves a write they never saw.
    Command {
        program: String,
        raw: String,
        cwd: String,
    },
    Text {
        text: String,
    },
}

/// How the interface answers a question.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "decision", rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum DecisionInput {
    Allow,
    /// Remembered against the subject for the rest of this session, in memory
    /// only. Nothing an approval does is written to configuration.
    AllowSession,
    /// The feedback becomes the tool result the model reads, so rejecting with
    /// a note is steering rather than a dead end.
    Deny {
        feedback: Option<String>,
    },
}

/// What `start_session` is given.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct StartSessionInput {
    /// Any path inside the repository. The repository root is resolved by git
    /// rather than by walking for a `.git`, because a worktree's `.git` is a
    /// file and a submodule's is too.
    pub path: String,
    pub prompt: Option<String>,
    /// `None` means whatever `[worktree]` resolved to — which is isolated
    /// unless the user turned it off. Never inferred from anything else.
    pub isolation: Option<Isolation>,
}

/// Everything the interface needs to paint itself once.
///
/// One call rather than four, so a first paint cannot show a project list from
/// one moment beside a session list from another.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct Snapshot {
    pub projects: Vec<ProjectView>,
    pub sessions: Vec<SessionView>,
    pub approvals: Vec<ApprovalView>,
    /// Absent when the supervisor could not open its index. The interface says
    /// so rather than showing an empty list, which would read as "no work".
    pub unavailable: Option<String>,
}

/// One error type, tagged, rather than a string.
///
/// A `Result<T, String>` forces the interface to match on prose to decide
/// whether something is retryable — which is how a wording change becomes a
/// behaviour change.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error, ts_rs::TS)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum AppError {
    #[error("{0}")]
    NoRepository(String),
    #[error("{0}")]
    NoSuchSession(String),
    #[error("{0}")]
    Supervisor(String),
    #[error("{0}")]
    Unavailable(String),
}

impl AppError {
    /// Whether trying the same thing again could plausibly work.
    ///
    /// Stated here so every surface agrees. A missing repository will still be
    /// missing; a supervisor that failed once might not fail twice.
    pub fn retryable(&self) -> bool {
        matches!(self, AppError::Supervisor(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The webview reads these keys. A rename that compiles is still a break,
    /// so the wire spelling is pinned rather than assumed.
    #[test]
    fn the_wire_shape_is_camel_case() {
        let view = SessionView {
            id: "01K".into(),
            short_id: "01K".into(),
            project_id: "p".into(),
            project_name: "n".into(),
            label: None,
            branch: None,
            workspace: "w".into(),
            isolation: Isolation::Worktree,
            status: SessionStatus::Running,
            started_ms: 1,
        };
        let json = serde_json::to_value(&view).expect("serialises");
        assert!(json.get("shortId").is_some(), "{json}");
        assert!(json.get("projectName").is_some(), "{json}");
        assert!(json.get("startedMs").is_some(), "{json}");
        assert_eq!(json["isolation"], "worktree");
        assert_eq!(json["status"], "running");
    }

    /// An error the interface can branch on without reading prose.
    #[test]
    fn an_error_carries_a_kind_and_says_whether_retrying_helps() {
        let json = serde_json::to_value(AppError::Supervisor("busy".into())).expect("serialises");
        assert_eq!(json["kind"], "supervisor");
        assert_eq!(json["message"], "busy");
        assert!(AppError::Supervisor(String::new()).retryable());
        assert!(!AppError::NoRepository(String::new()).retryable());
    }

    #[test]
    fn a_decision_round_trips_with_its_feedback() {
        let denied = DecisionInput::Deny {
            feedback: Some("use the existing helper".into()),
        };
        let json = serde_json::to_value(&denied).expect("serialises");
        assert_eq!(json["decision"], "deny");
        let back: DecisionInput = serde_json::from_value(json).expect("round trips");
        assert!(matches!(back, DecisionInput::Deny { feedback: Some(_) }));
    }
}
