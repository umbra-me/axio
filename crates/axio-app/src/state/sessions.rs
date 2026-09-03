//! The session lifecycle, as the interface reaches it.
//!
//! Start, follow up, stop, close, and what changed — each one the command the
//! command line already has. Kept beside `mod.rs` rather than in it so the
//! parent stays the one place the struct and its projections are defined.

use std::path::Path;

use axio_supervisor::{Disposition, Isolation as SupervisorIsolation, StartOptions};

use super::{AppState, short};
use crate::model::{AppError, Isolation, SessionStatus, SessionView, StartSessionInput};

impl AppState {
    /// Start a session, and run a turn in it when a prompt was given.
    ///
    /// The turn is spawned rather than awaited. A command that blocked until a
    /// model finished would hold the interface for minutes, and Tauri runs a
    /// command's future on a worker only if it is declared async — a detail the
    /// prior art gets wrong on all nine of its commands, three of which do
    /// seconds of blocking work on the thread that paints.
    pub async fn start_session(&self, input: StartSessionInput) -> Result<SessionView, AppError> {
        let supervisor = self.supervisor()?;
        let handle = supervisor
            .start(
                Path::new(&input.path),
                StartOptions {
                    isolation: input.isolation.map(|i| match i {
                        Isolation::Worktree => SupervisorIsolation::Worktree,
                        Isolation::Direct => SupervisorIsolation::Direct,
                    }),
                    label: input.prompt.clone(),
                    group: input.group.clone(),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| match e {
                axio_supervisor::SupervisorError::NotARepository(_) => {
                    AppError::NoRepository(e.to_string())
                }
                other => AppError::Supervisor(other.to_string()),
            })?;

        if let Some(prompt) = input.prompt.clone() {
            let handle = handle.clone();
            tokio::spawn(async move {
                let _ = handle.turn(prompt).await;
            });
        }

        let entry = supervisor
            .history()
            .into_iter()
            .find(|e| e.session == handle.session);

        Ok(SessionView {
            id: handle.session.to_string(),
            short_id: short(&handle.session.to_string()),
            project_id: handle.project.to_string(),
            project_name: entry
                .as_ref()
                .map(|e| e.project_name.clone())
                .unwrap_or_default(),
            label: input.prompt,
            title: None,
            group: input.group,
            branch: handle.checkout.branch.clone(),
            workspace: handle.checkout.path.display().to_string(),
            isolation: match handle.checkout.isolation {
                SupervisorIsolation::Worktree => Isolation::Worktree,
                SupervisorIsolation::Direct => Isolation::Direct,
            },
            status: SessionStatus::Running,
            open: true,
            started_ms: entry.map(|e| e.started_ms).unwrap_or_default(),
        })
    }

    /// Give a session a name, or take it away with `None`.
    pub fn rename_session(&self, session_id: &str, title: Option<String>) -> Result<(), AppError> {
        let supervisor = self.supervisor()?;
        let id = session_id
            .parse()
            .map_err(|_| AppError::NoSuchSession(format!("`{session_id}` is not a session id")))?;
        supervisor
            .rename(id, title)
            .map_err(|e| AppError::Supervisor(e.to_string()))
    }

    /// Send a prompt to a session that already exists.
    pub async fn send(&self, session_id: &str, prompt: String) -> Result<(), AppError> {
        let supervisor = self.supervisor()?;
        let id = session_id
            .parse()
            .map_err(|_| AppError::NoSuchSession(format!("`{session_id}` is not a session id")))?;
        let handle = supervisor
            .session(id)
            .map_err(|e| AppError::NoSuchSession(e.to_string()))?;
        tokio::spawn(async move {
            let _ = handle.turn(prompt).await;
        });
        Ok(())
    }

    /// Interrupt whatever a session is doing, without closing it.
    pub fn cancel(&self, session_id: &str) -> Result<(), AppError> {
        let supervisor = self.supervisor()?;
        let id = session_id
            .parse()
            .map_err(|_| AppError::NoSuchSession(format!("`{session_id}` is not a session id")))?;
        supervisor
            .session(id)
            .map_err(|e| AppError::NoSuchSession(e.to_string()))?
            .cancel();
        Ok(())
    }

    /// End a session. `discard` also removes its worktree and branch.
    pub async fn close(&self, session_id: &str, discard: bool) -> Result<(), AppError> {
        let supervisor = self.supervisor()?;
        let id = session_id
            .parse()
            .map_err(|_| AppError::NoSuchSession(format!("`{session_id}` is not a session id")))?;
        supervisor
            .close(
                id,
                if discard {
                    Disposition::Discard
                } else {
                    Disposition::Keep
                },
            )
            .await
            .map_err(|e| AppError::Supervisor(e.to_string()))
    }

    /// What a session changed, as a unified diff.
    pub async fn diff(&self, session_id: &str) -> Result<String, AppError> {
        let supervisor = self.supervisor()?;
        let id: axio_core::protocol::SessionId = session_id
            .parse()
            .map_err(|_| AppError::NoSuchSession(format!("`{session_id}` is not a session id")))?;
        let entry = supervisor
            .history()
            .into_iter()
            .find(|e| e.session == id)
            .ok_or_else(|| AppError::NoSuchSession(format!("no session {session_id}")))?;
        entry
            .checkout()
            .diff()
            .await
            .map_err(|e| AppError::Supervisor(e.to_string()))
    }
}
