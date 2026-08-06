//! The axio desktop surface.
//!
//! A view of `axio-supervisor`, not a second product. Everything it can do,
//! `axio session` can do first — that ordering is the point, because a surface
//! holding a capability the command line lacks has stopped being the same
//! thing wearing a different interface.
//!
//! Four rules hold it up.
//!
//! 1. **Rust owns all state; TypeScript owns pixels.** No session store, no
//!    settings schema and no reconciliation logic in the webview. The process
//!    that owns the sessions owns the record of them, so a restart is an
//!    internal invariant rather than a diff between two disagreeing stores.
//! 2. **Every command is `async`.** A Tauri command declared without it runs on
//!    the thread that paints. The prior art declares all nine of its commands
//!    sync, and three of them do seconds of blocking work — process probes at
//!    startup, a three-second-per-session teardown spin on quit.
//! 3. **The webview is granted almost nothing.** Window controls route through
//!    one typed command rather than through capabilities, which keeps a Rust
//!    chokepoint that can refuse; and a real CSP is set, which the prior art
//!    disables outright.
//! 4. **State is testable without a webview.** [`state`] and [`model`] link no
//!    Tauri at all, so the whole surface behind the glass is exercised by
//!    ordinary unit tests. Only [`commands`] sits behind the `app` feature.

#![forbid(unsafe_code)]

pub mod model;
pub mod state;

#[cfg(feature = "app")]
pub mod commands;

#[cfg(feature = "app")]
mod shell;

pub use model::{
    AppError, ApprovalView, DecisionInput, Isolation, PreviewView, ProjectView, SessionStatus,
    SessionView, Snapshot, StartSessionInput,
};
pub use state::AppState;

#[cfg(feature = "app")]
pub use shell::run;

/// The state the shipped binary runs on.
///
/// Kept here rather than in the binary so the wiring is one line there and
/// testable here. The supervisor's root is under `axio_home` and not the state
/// directory, for the reason the CLI records: the state directory is per
/// process, and a worktree cut into one is a branch the next run cannot find.
#[cfg(feature = "app")]
pub fn shell_state() -> AppState {
    use axio_supervisor::{Supervisor, SupervisorConfig};

    let root = match std::env::var_os("AXIO_HOME") {
        Some(home) => std::path::PathBuf::from(home),
        None => match std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            Some(home) => std::path::PathBuf::from(home).join(".axio"),
            None => std::path::PathBuf::from(".axio"),
        },
    }
    .join("supervisor");

    // A factory that starts nothing. The shell reads and reviews; running a
    // turn needs the wiring `axio` owns, and reaching for it from here would be
    // the second copy of `prepare` this whole architecture exists to avoid.
    struct ReadOnly;
    #[async_trait::async_trait]
    impl axio_supervisor::AgentFactory for ReadOnly {
        async fn build(
            &self,
            _request: axio_supervisor::AgentRequest,
        ) -> Result<axio_core::Agent, String> {
            Err("this build of the shell reviews sessions but does not start them".to_owned())
        }
    }

    match Supervisor::new(
        SupervisorConfig {
            state_root: root,
            worktree: axio_core::config::WorktreeSection::default(),
        },
        std::sync::Arc::new(ReadOnly),
    ) {
        Ok((supervisor, _events)) => AppState::new(std::sync::Arc::new(supervisor)),
        Err(e) => AppState::unavailable(e.to_string()),
    }
}
