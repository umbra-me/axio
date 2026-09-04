//! What crosses to the webview about a hosted terminal, and where one runs.

use serde::{Deserialize, Serialize};

use crate::model::Isolation;

/// A hosted agent, as a list row sees it.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct HostedView {
    pub id: String,
    pub harness: String,
    /// What the harness is called — `Claude Code`. The same for every one.
    pub label: String,
    /// What *this* one is called — `Claude Code 2` while a first is live. Two
    /// terminals with one name are two things a person cannot tell apart from
    /// the rail, which is the whole reason a rail lists them.
    pub name: String,
    /// The branch its worktree is on, when it has one of its own.
    pub branch: Option<String>,
    /// Started together with others; see `StartGroupInput`.
    pub group: Option<String>,
    /// The CSS custom property this harness is coloured with. Decided in Rust
    /// beside the harness list, so a colour and the thing it identifies cannot
    /// be two lists that disagree.
    pub accent_var: String,
    pub cwd: String,
    /// The repository its directory belongs to — the checkout itself when
    /// direct. What lists a terminal under its repository beside the
    /// sessions, rather than in a pile of its own.
    pub repo: String,
    pub status: String,
    /// Set only once it has stopped.
    pub exit_code: Option<i32>,
    /// What the agent itself says it is doing, from its hooks or an in-band
    /// status sequence: `working`, `blocked` (waiting on a permission),
    /// `idle` (waiting on a prompt), `done`. `None` for a tool that has no
    /// way to say, and then a surface falls back to watching its output.
    pub agent_status: Option<String>,
    /// The tool's own session id, learned from its hooks, which makes a
    /// resume exact and its transcript readable.
    pub provider_session: Option<String>,
    /// `pty` — the tool as itself in a terminal — or `app`: driven through
    /// its own structured protocol, with no terminal to draw.
    pub transport: String,
    /// Stopped by a person, rather than by the window closing or the tool
    /// exiting on its own. A row that was stopped on purpose stays stopped:
    /// the next window lists it and waits to be asked, where one that was
    /// merely interrupted comes back running.
    pub stopped: bool,
}

/// What starting one takes.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct StartHostedInput {
    /// One of the allowlisted names. Never a command line.
    pub harness: String,
    /// The repository to work in. Where the agent actually runs depends on
    /// `isolation`: its own worktree cut from here, or this directory itself.
    pub cwd: String,
    /// Its own worktree, or the checkout as it sits. `None` is the worktree —
    /// the same default a session has, for the same reason: several agents on
    /// one checkout is one corrupted tree.
    #[serde(default)]
    pub isolation: Option<Isolation>,
    /// Membership of a group started together.
    #[serde(default)]
    pub group: Option<String>,
    /// Extra arguments, split the way a shell would split them without one
    /// running. Empty is the normal case.
    #[serde(default)]
    pub args: String,
    /// The size of the pane the terminal is about to appear in.
    ///
    /// Sent at start rather than only on the resize that follows, because a
    /// harness paints its opening screen from the size it is given and that
    /// paint lands in scrollback permanently. Started at a guess and corrected
    /// a moment later, the correction repaints the live area and leaves the
    /// mis-sized opening above it forever.
    /// A first prompt, handed to the tool on its command line where it
    /// takes one (see `Harness::prompt_args`); a caller types it in
    /// otherwise. Never split: a prompt is one argument however it reads.
    #[serde(default)]
    pub prompt: Option<String>,
    /// `app` to drive the tool through its structured protocol where it has
    /// one (Codex's app-server); anything else is a terminal.
    #[serde(default)]
    pub transport: Option<String>,
    /// For the structured transport, which passes these as protocol fields
    /// rather than command-line flags.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub rows: Option<u16>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub cols: Option<u16>,
}

/// Everything a read returns: the bytes, and where to ask from next.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct HostedOutput {
    /// Decoded here rather than in the webview, because this side holds the
    /// whole stream and can decode across the chunk boundaries a `read` lands
    /// on. Lossy only at the very end, where a trailing partial character is
    /// genuinely incomplete rather than merely split.
    pub text: String,
    /// `number`, not `bigint`. ts-rs maps `u64` to `bigint` by default, which
    /// would be right for a boundary that preserved 64-bit integers — and this
    /// one does not: Tauri's IPC is JSON, so what actually arrives is a JS
    /// number. Declaring `bigint` would be a type that never matches the value.
    /// Both quantities here are safe below 2^53: a millisecond timestamp until
    /// the year 287396, and a byte cursor until nine petabytes through one
    /// terminal.
    #[ts(type = "number")]
    pub cursor: u64,
}

/// Where a hosted agent runs: the directory, and the branch if the directory
/// is a worktree cut for it. Decided by the state, which has the supervisor;
/// this module only spawns where it is told.
#[derive(Debug, Clone)]
pub struct Place {
    pub cwd: std::path::PathBuf,
    /// The repository the directory belongs to — itself, when direct.
    pub repo: std::path::PathBuf,
    pub branch: Option<String>,
}
