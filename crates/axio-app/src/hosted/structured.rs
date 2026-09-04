//! Rows driven through a tool's own protocol rather than a terminal.
//!
//! The registry lists them beside the terminals — same ids, same journal,
//! same rail — and the difference is what is behind the row: a
//! `CodexSession` speaking JSON-RPC instead of a pty. What a terminal
//! answers with bytes, these answer with rows and questions.

use std::sync::Arc;

use axio_pty::Harness;

use super::appserver::{CodexSession, HostedApproval};
use super::{Held, Hosted, HostedView, Place, StartHostedInput, view_of};
use crate::model::AppError;

impl Hosted {
    /// Start a structured row. Only Codex has the protocol; anything else
    /// asked for it is refused rather than quietly given a terminal.
    pub async fn start_app(
        &self,
        input: StartHostedInput,
        place: Place,
        on_activity: impl Fn(String) + Send + 'static,
    ) -> Result<HostedView, AppError> {
        let harness = Harness::parse(&input.harness).ok_or_else(|| {
            AppError::NoRepository(format!("`{}` is not an agent axio can host", input.harness))
        })?;
        if harness != Harness::Codex {
            return Err(AppError::Supervisor(format!(
                "{} has no structured protocol; host it in a terminal",
                harness.label()
            )));
        }
        let extra = axio_pty::split_args(&input.args).map_err(AppError::Supervisor)?;
        let app = CodexSession::spawn(
            &place.cwd,
            input.permission.as_deref(),
            input.model.clone(),
            input.effort.clone(),
            &extra,
            None,
        )
        .await?;
        let id = self.mint();
        let view = {
            let mut held = self
                .sessions
                .lock()
                .expect("no lock is held across an await");
            let entry = Held {
                harness,
                live: None,
                app: Some(Arc::clone(&app)),
                transport: "app".to_owned(),
                number: Self::number_for(held.values().map(|h| (h.harness, h.number)), harness),
                group: input.group.clone(),
                title: None,
                place,
                args: input.args.clone(),
                stopped: false,
                agent: super::agent::AgentState::default(),
            };
            let view = view_of(&id, &entry);
            held.insert(id.clone(), entry);
            self.record(&held);
            view
        };
        Self::relay_app(Arc::clone(&app), id.clone(), on_activity);
        if let Some(prompt) = input.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
            app.prompt(prompt).await?;
        }
        Ok(view)
    }

    /// Start the protocol again for a row that had it, with the thread it
    /// had, to be resumed by the next prompt.
    pub(super) async fn resume_app(&self, id: &str) -> Result<HostedView, AppError> {
        let (place, args, thread) = {
            let held = self
                .sessions
                .lock()
                .expect("no lock is held across an await");
            let entry = held
                .get(id)
                .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?;
            let thread = entry
                .app
                .as_ref()
                .and_then(|a| a.thread())
                .or_else(|| entry.agent.session.clone());
            (entry.place.clone(), entry.args.clone(), thread)
        };
        if !place.cwd.is_dir() {
            return Err(AppError::Supervisor(format!(
                "its directory is gone: {}. Remove it from the list, or start a new one.",
                place.cwd.display()
            )));
        }
        let extra = axio_pty::split_args(&args).map_err(AppError::Supervisor)?;
        let app = CodexSession::spawn(&place.cwd, None, None, None, &extra, thread.clone()).await?;
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        let entry = held
            .get_mut(id)
            .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?;
        entry.app = Some(app);
        entry.agent.session = thread;
        entry.stopped = false;
        Ok(view_of(id, entry))
    }

    /// A line to a structured row is a prompt; to a terminal it is typed.
    pub async fn say(&self, id: &str, text: String) -> Result<(), AppError> {
        match self.app(id)? {
            Some(app) => app.prompt(&text).await,
            None => self.submit(id, text),
        }
    }

    pub fn approvals(&self, id: &str) -> Result<Vec<HostedApproval>, AppError> {
        Ok(self.app(id)?.map(|a| a.approvals()).unwrap_or_default())
    }

    pub async fn decide(&self, id: &str, approval: &str, decision: &str) -> Result<(), AppError> {
        match self.app(id)? {
            Some(app) => app.decide(approval, decision).await,
            None => Err(AppError::Supervisor(
                "that terminal takes its answers in the terminal".to_owned(),
            )),
        }
    }

    /// The structured row's transcript, or the terminal's from its file.
    pub fn any_transcript(&self, id: &str) -> Result<crate::model::TranscriptView, AppError> {
        match self.app(id)? {
            Some(app) => Ok(app.transcript()),
            None => self.transcript(id),
        }
    }
}
