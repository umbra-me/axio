//! What can go wrong before a session exists.
//!
//! Once a session is running its failures are `TurnOutcome`s and travel on the
//! event stream. Everything here happens earlier — finding the repository,
//! cutting the worktree, building the agent — where there is no session to
//! carry a notice yet.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    /// The path is not inside a git repository.
    ///
    /// Not downgraded to "run without a worktree": isolation was asked for and
    /// silently not getting it is the one outcome nobody would notice.
    #[error("{0} is not inside a git repository, so a session cannot be isolated in a worktree")]
    NotARepository(PathBuf),

    /// A repository with no commits has nothing to branch from.
    #[error(
        "{0} has no commits yet, so there is nothing for a worktree to branch from; \
         make one commit, or start this session with isolation turned off"
    )]
    NoCommits(PathBuf),

    #[error("git {args}: {message}")]
    Git { args: String, message: String },

    #[error("could not run git: {0}. Is it installed and on PATH?")]
    GitMissing(String),

    #[error("no session {0}")]
    NoSuchSession(axio_core::protocol::SessionId),

    #[error("no project {0}")]
    NoSuchProject(String),

    /// The session's task is gone — it panicked, or it was already closed.
    #[error("session {0} is no longer running")]
    SessionGone(axio_core::protocol::SessionId),

    /// The factory could not build an agent. Its own message, verbatim.
    #[error("could not start a session: {0}")]
    Factory(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl SupervisorError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, SupervisorError>;
