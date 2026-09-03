//! Everything the application knows.
//!
//! Rust owns all of it. The webview is sent projections and given no way to
//! hold state of its own — no session list in a store, no settings schema in
//! TypeScript, no reconciliation between what the interface believes and what
//! is actually running.
//!
//! That is a deliberate correction. The prior art keeps its durable records in
//! `localStorage` while the live PTYs live in Rust, which means a restart needs
//! a cross-language diff — a whole module whose only job is deciding which of
//! two disagreeing stores is right. Here the process that owns the sessions is
//! the process that owns the record of them, so reconciliation is an internal
//! invariant rather than a negotiation.
//!
//! Nothing in this module depends on Tauri, which is what lets it be tested
//! without a webview and driven from a terminal.

mod sessions;

use std::path::Path;
use std::sync::Arc;

use axio_core::record::SessionStore;
use axio_supervisor::{
    Isolation as SupervisorIsolation, SessionStatus as SupervisorStatus, SupervisedEvent,
    Supervisor,
};

use crate::model::{
    AppError, ApprovalView, DecisionInput, Isolation, ProjectView, SessionStatus, SessionView,
    Snapshot, TranscriptView,
};
use crate::transcript::{Transcripts, preview_of};

/// The application's state.
///
/// A supervisor and nothing else, so far, and that is the point: every question
/// the interface can ask is answered from the same place the CLI answers it
/// from. When settings arrive they arrive here, not in a TypeScript module.
pub struct AppState {
    /// Other agents' CLIs, in terminals this process owns. Beside the
    /// supervisor rather than inside it: `axio-supervisor` runs axio's own
    /// loop, and a hosted Claude Code is not that — it has its own approvals,
    /// its own session and its own interface, and pretending otherwise would
    /// mean modelling it as something it is not.
    pub hosted: crate::hosted::Hosted,
    /// What every session has said and done, as seen from the event stream.
    transcripts: Transcripts,
    /// Where session files live, for a transcript this process did not watch.
    /// Absent in tests, and then a session another process ran has no
    /// transcript here — which the view says, rather than showing nothing.
    store: Option<SessionStore>,
    supervisor: Option<Arc<Supervisor>>,
    /// Why there is no supervisor, when there is none. Kept so the interface
    /// can say what is wrong instead of showing an empty list — "no work" and
    /// "could not look" are different answers and must not look alike.
    unavailable: Option<String>,
}

impl AppState {
    pub fn new(supervisor: Arc<Supervisor>) -> Self {
        Self {
            hosted: crate::hosted::Hosted::default(),
            transcripts: Transcripts::default(),
            store: None,
            supervisor: Some(supervisor),
            unavailable: None,
        }
    }

    /// Read transcripts this process did not watch from the session files.
    pub fn with_store(mut self, store: SessionStore) -> Self {
        self.store = Some(store);
        self
    }

    /// A state that can still paint, for when the index could not be opened.
    pub fn unavailable(why: impl Into<String>) -> Self {
        Self {
            hosted: crate::hosted::Hosted::default(),
            transcripts: Transcripts::default(),
            store: None,
            supervisor: None,
            unavailable: Some(why.into()),
        }
    }

    /// Fold one event from the supervisor into the transcript it belongs to.
    ///
    /// Called by whatever relays the stream to the window, before the window
    /// is told — so by the time it asks, the answer is already here.
    pub fn observe(&self, event: &SupervisedEvent) {
        self.transcripts.observe(&event.event);
    }

    fn supervisor(&self) -> Result<&Arc<Supervisor>, AppError> {
        self.supervisor.as_ref().ok_or_else(|| {
            AppError::Unavailable(
                self.unavailable
                    .clone()
                    .unwrap_or_else(|| "sessions are unavailable".to_owned()),
            )
        })
    }

    /// Everything the interface needs to paint itself, in one read.
    ///
    /// One call rather than three, because three would let a first paint show a
    /// project list from one moment beside a session list from another — and
    /// the mismatch would look like a bug in the work rather than in the paint.
    pub fn snapshot(&self) -> Snapshot {
        let Ok(supervisor) = self.supervisor() else {
            return Snapshot {
                projects: Vec::new(),
                sessions: Vec::new(),
                approvals: Vec::new(),
                unavailable: self.unavailable.clone(),
            };
        };

        let history = supervisor.history();
        let live = supervisor.sessions();

        let sessions: Vec<SessionView> = history
            .iter()
            .map(|entry| {
                let handle = live.iter().find(|h| h.session == entry.session);
                SessionView {
                    id: entry.session.to_string(),
                    short_id: short(&entry.session.to_string()),
                    project_id: entry.project.to_string(),
                    project_name: entry.project_name.clone(),
                    label: entry.label.clone(),
                    branch: entry.branch.clone(),
                    workspace: entry.workspace.display().to_string(),
                    isolation: match entry.isolation {
                        SupervisorIsolation::Worktree => Isolation::Worktree,
                        SupervisorIsolation::Direct => Isolation::Direct,
                    },
                    // Live state outranks the index: the index records that a
                    // session was started, and only the handle knows whether a
                    // turn is in flight right now.
                    status: match handle.map(|h| h.status()) {
                        Some(SupervisorStatus::Running) => SessionStatus::Running,
                        Some(SupervisorStatus::Idle) => SessionStatus::Idle,
                        None => SessionStatus::Closed,
                    },
                    open: entry.is_open(),
                    started_ms: entry.started_ms,
                }
            })
            .collect();

        let projects: Vec<ProjectView> = supervisor
            .projects()
            .into_iter()
            .map(|project| {
                let id = project.id.to_string();
                let mine: Vec<&SessionView> =
                    sessions.iter().filter(|s| s.project_id == id).collect();
                ProjectView {
                    open_sessions: mine
                        .iter()
                        .filter(|s| s.status != SessionStatus::Closed)
                        .count(),
                    total_sessions: mine.len(),
                    id,
                    name: project.name,
                    root: project.root.display().to_string(),
                }
            })
            .collect();

        Snapshot {
            projects,
            sessions,
            approvals: self.approvals(),
            unavailable: None,
        }
    }

    /// Every question waiting on an answer, oldest first.
    pub fn approvals(&self) -> Vec<ApprovalView> {
        let Ok(supervisor) = self.supervisor() else {
            return Vec::new();
        };
        supervisor
            .pending_approvals()
            .into_iter()
            .map(|pending| ApprovalView {
                id: pending.id.to_string(),
                session_id: pending.session.to_string(),
                short_session_id: short(&pending.session.to_string()),
                project_id: pending.project.to_string(),
                subject: pending.request.subject.clone(),
                tool: pending.request.tool.clone(),
                reason: pending.request.reason.clone(),
                preview: pending.request.preview.as_ref().map(preview_of),
                at_ms: pending.at_ms,
            })
            .collect()
    }

    /// Register a repository without starting anything in it.
    ///
    /// What "add a repository" means here: the window lists projects that have
    /// work, and a freshly picked directory has none yet. Registering it puts
    /// it in the list so the composer can be pointed at it. Nothing is written
    /// — a project with no sessions is forgotten on restart, deliberately, for
    /// the reason `axio_supervisor::Projects` gives: a second list on disk is a
    /// list that can disagree with the index.
    pub async fn open_project(&self, path: &str) -> Result<ProjectView, AppError> {
        let supervisor = self.supervisor()?;
        let project = supervisor
            .open_project(Path::new(path))
            .await
            .map_err(|e| match e {
                axio_supervisor::SupervisorError::NotARepository(_) => {
                    AppError::NoRepository(e.to_string())
                }
                other => AppError::Supervisor(other.to_string()),
            })?;
        let id = project.id.to_string();
        let mine: Vec<SessionView> = self
            .snapshot()
            .sessions
            .into_iter()
            .filter(|s| s.project_id == id)
            .collect();
        Ok(ProjectView {
            open_sessions: mine
                .iter()
                .filter(|s| s.status != SessionStatus::Closed)
                .count(),
            total_sessions: mine.len(),
            id,
            name: project.name,
            root: project.root.display().to_string(),
        })
    }

    /// What a session has said and done.
    ///
    /// Live sessions answer from what the event stream delivered. Any other —
    /// one the command line ran, or one that ended before this window opened —
    /// is read from its file the first time it is asked for, and the view says
    /// which it was.
    pub fn transcript(&self, session_id: &str) -> Result<TranscriptView, AppError> {
        let supervisor = self.supervisor()?;
        let id: axio_core::protocol::SessionId = session_id
            .parse()
            .map_err(|_| AppError::NoSuchSession(format!("`{session_id}` is not a session id")))?;
        let live = supervisor.session(id).is_ok();
        if !live && !self.transcripts.knows(id) {
            supervisor
                .history()
                .into_iter()
                .find(|e| e.session == id)
                .ok_or_else(|| AppError::NoSuchSession(format!("no session {session_id}")))?;
        }
        if let Some(store) = &self.store {
            let path = store.path_for(id);
            if path.exists()
                && let Ok(loaded) = axio_core::record::load(&path)
            {
                self.transcripts.seed(
                    id,
                    Some(loaded.header.model.clone()),
                    loaded.session.transcript(),
                );
            }
        }
        Ok(self.transcripts.view(id, !live))
    }

    /// Answer a question. `false` if it had already been answered.
    pub fn resolve_approval(
        &self,
        approval_id: &str,
        decision: DecisionInput,
    ) -> Result<bool, AppError> {
        let supervisor = self.supervisor()?;
        let id = approval_id.parse().map_err(|_| {
            AppError::NoSuchSession(format!("`{approval_id}` is not an approval id"))
        })?;
        Ok(supervisor.resolve_approval(id, decision.into()))
    }
}

impl From<DecisionInput> for axio_core::protocol::Decision {
    fn from(input: DecisionInput) -> Self {
        match input {
            DecisionInput::Allow => Self::Allow,
            DecisionInput::AllowSession => Self::AllowSession,
            DecisionInput::Deny { feedback } => Self::Deny { feedback },
        }
    }
}

/// Eight characters of a ULID — the same prefix the CLI prints and accepts.
fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A surface that cannot supervise still has to paint. Showing an empty
    /// list would read as "no work", which is a different and wrong answer.
    #[test]
    fn an_unavailable_state_says_so_rather_than_looking_empty() {
        let state = AppState::unavailable("the index could not be opened");
        let snapshot = state.snapshot();
        assert!(snapshot.projects.is_empty());
        assert!(snapshot.sessions.is_empty());
        assert_eq!(
            snapshot.unavailable.as_deref(),
            Some("the index could not be opened")
        );
    }

    #[tokio::test]
    async fn every_action_on_an_unavailable_state_is_an_error_not_a_panic() {
        let state = AppState::unavailable("nope");
        assert!(state.cancel("01K").is_err());
        assert!(state.close("01K", false).await.is_err());
        assert!(state.diff("01K").await.is_err());
        assert!(state.resolve_approval("01K", DecisionInput::Allow).is_err());
        assert!(state.send("01K", "hi".into()).await.is_err());
    }

    /// A malformed id is a bad request, not a missing session, and definitely
    /// not a panic in a command handler.
    #[tokio::test]
    async fn a_malformed_session_id_is_reported_rather_than_parsed_hopefully() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let (supervisor, _events) = Supervisor::new(
            axio_supervisor::SupervisorConfig {
                state_root: dir.path().to_path_buf(),
                worktree: axio_core::config::WorktreeSection::default(),
            },
            Arc::new(NoFactory),
        )
        .expect("a supervisor");
        let state = AppState::new(Arc::new(supervisor));

        match state.cancel("not-an-id") {
            Err(AppError::NoSuchSession(message)) => assert!(message.contains("not-an-id")),
            other => panic!("expected NoSuchSession, got {other:?}"),
        }
    }

    #[test]
    fn a_short_id_is_eight_characters() {
        assert_eq!(short("01KZC1YE19PHWPQ4E5SS9PM786"), "01KZC1YE");
    }

    struct NoFactory;
    #[async_trait::async_trait]
    impl axio_supervisor::AgentFactory for NoFactory {
        async fn build(
            &self,
            _request: axio_supervisor::AgentRequest,
        ) -> Result<axio_core::Agent, String> {
            Err("no agents in this test".to_owned())
        }
    }
}
