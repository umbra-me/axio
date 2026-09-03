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
//! The TypeScript for every shape here is generated, not mirrored: ts-rs
//! writes `ui/src/generated/` when this crate's tests run, and a Rust change
//! with no regeneration shows up as a dirty tree. The prior art maintains both
//! sides by hand and warns, in four separate documents, that "changing one
//! side without the other is a silent break" — and it drifted anyway. A
//! warning repeated four times is a job for a build step, and it is one.

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
    /// A name a person gave it later. Shown in place of the label when set.
    pub title: Option<String>,
    /// Sessions started together share one; see `StartGroupInput`.
    pub group: Option<String>,
    pub branch: Option<String>,
    pub workspace: String,
    pub isolation: Isolation,
    pub status: SessionStatus,
    /// Whether the index still counts it as open. `Closed` above means "not
    /// live in this process", which a session started from the command line
    /// also is; this says whether anyone has actually closed it.
    pub open: bool,
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
    /// Membership of a group started together. Set by `start_group`, not by a
    /// person: a group is a fact about how sessions began.
    #[serde(default)]
    pub group: Option<String>,
}

/// One prompt, several agents.
///
/// `count` axio sessions and one hosted terminal per name in `agents`, all on
/// the same repository, each in a worktree of its own, all tagged with one
/// group id so the rail can show them together and a compare view can lay
/// their diffs side by side. Nothing else is shared: each member has its own
/// approvals, transcript and branch, and is closed on its own.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct StartGroupInput {
    pub path: String,
    pub prompt: String,
    /// How many axio sessions. Zero is allowed when `agents` is not empty.
    pub count: u32,
    /// Harness names — `claude`, `codex`, `pi`, `axio` — one terminal each.
    #[serde(default)]
    pub agents: Vec<String>,
}

/// One more member for a group that already exists.
///
/// `harness` names a hosted agent; `None` means an axio session. The prompt
/// is the group's — the interface has it from any session member — and is
/// what the newcomer is asked, so it joins the same work rather than an
/// empty terminal.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AddToGroupInput {
    pub group: String,
    pub path: String,
    pub prompt: Option<String>,
    pub harness: Option<String>,
}

/// What `start_group` returns: the id every member carries, and the members.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct GroupStart {
    pub group: String,
    pub sessions: Vec<SessionView>,
    pub terminals: Vec<crate::hosted::HostedView>,
}

/// A provider axio knows, and whether a credential for it is on this machine.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ProviderView {
    pub name: String,
    pub ready: bool,
}

/// Where a session's work stands relative to the repository it came from.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct LandingView {
    pub branch: Option<String>,
    /// The branch the repository itself is on — what a merge lands into.
    pub base: String,
    /// `git status --porcelain` in the worktree, one line per path.
    pub changed: Vec<String>,
    /// Commits on the branch the base does not have.
    pub ahead: u32,
    /// `origin`, when the repository has one.
    pub remote: Option<String>,
    /// Whether `gh` is on this machine, so a pull request can be opened.
    pub can_pr: bool,
}

/// The three ways this window lands work. A workflow choice the supervisor
/// deliberately leaves to the surface; here they are, spelled out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum LandAction {
    /// Commit what is uncommitted, then merge the branch into the repository's
    /// current branch with a merge commit.
    Merge,
    /// Commit what is uncommitted, then push the branch to `origin`.
    Push,
    /// Push, then open a pull request with `gh`.
    PullRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct LandOutcome {
    pub message: String,
    /// A pull request's address, when one was opened.
    pub url: Option<String>,
}

/// One row of a session's transcript, in the shape a reader needs.
///
/// A projection of `axio_core::protocol::Item`, flattened: the tool call's
/// status and its output are fields rather than a nested enum, because a row
/// wants "is it done, and what did it say" and not the wire's state machine.
/// Turn boundaries and notices are rows too, so the whole thing is one list
/// in the order it happened.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    export,
    export_to = "../ui/src/generated/",
    rename_all_fields = "camelCase"
)]
pub enum TranscriptEntry {
    User {
        id: String,
        text: String,
    },
    Agent {
        id: String,
        text: String,
        /// Still arriving. A reader can show a cursor; nothing else changes.
        streaming: bool,
    },
    Reasoning {
        id: String,
        text: String,
    },
    Tool {
        id: String,
        name: String,
        subject: String,
        /// `pending`, `awaitingApproval`, `running`, `ok`, `failed`, `denied`
        /// or `cancelled` — the wire's own status names.
        status: String,
        /// The tool's output when it ran, or the message when it did not.
        output: String,
        truncated: bool,
        preview: Option<PreviewView>,
        #[ts(type = "number")]
        ms: u64,
    },
    Interrupted {
        id: String,
        after_steps: u32,
    },
    Elision {
        id: String,
        dropped_items: u32,
    },
    /// A turn ended. `outcome` is the wire's snake_case tag: `completed`,
    /// `refused`, `interrupted`, `step_limit`, `budget_exceeded`, `failed`.
    Turn {
        id: String,
        outcome: String,
        detail: String,
        cost_usd: f64,
    },
    Notice {
        id: String,
        level: String,
        message: String,
    },
}

/// A session's transcript, as far as this process has seen it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TranscriptView {
    pub entries: Vec<TranscriptEntry>,
    pub model: Option<String>,
    pub cost_usd: f64,
    /// Read back from the session file rather than watched live. True for a
    /// session another process ran, or one that ended before this window
    /// opened; the transcript is complete up to the last record written.
    pub from_record: bool,
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
            title: None,
            group: None,
            branch: None,
            workspace: "w".into(),
            isolation: Isolation::Worktree,
            status: SessionStatus::Running,
            open: true,
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
