//! Bringing a terminal back, stopping it, and forgetting it.
//!
//! Three verbs with three different scopes. Stop ends the process and keeps
//! the row, because the row is what a resume needs. Kill ends the process and
//! the row, and the journal with it. Resume is a new process in the old
//! place, asked to pick up where the previous one left off.

use std::sync::Arc;

use axio_pty::{Harness, HarnessSession, HarnessStatus, split_args};

use super::{Hosted, HostedView, appserver, view_of};
use crate::model::AppError;

impl Hosted {
    /// Relay a live terminal's "something happened" signal to `on_activity`
    /// until it stops.
    ///
    /// The callback is given the session id and nothing else, deliberately.
    /// What it is for is telling a surface to *ask*, not telling it what
    /// changed — the bytes still come back through a cursor, so a listener that
    /// missed a signal is late rather than wrong. The loop ends with the
    /// process: the exit is signalled too, and after it there is nothing to
    /// relay.
    pub(super) fn relay(
        session: Arc<HarnessSession>,
        id: String,
        on_activity: impl Fn(String) + Send + 'static,
    ) {
        let wrote = session.wrote();
        tokio::spawn(async move {
            loop {
                // Registered before the wait, which is the whole discipline
                // of `Notify`: created afterwards, output landing in the gap
                // would wake nobody.
                let waiting = wrote.notified();
                waiting.await;
                on_activity(id.clone());
                if session.status() != HarnessStatus::Running {
                    break;
                }
            }
        });
    }

    /// Anything the terminal said in-band since last asked, applied to its
    /// row. Called by whoever reads output, so a status printed by the
    /// program is known by the time the surface asks.
    pub fn observe_signals(&self, id: &str) {
        let Ok(session) = self.get(id) else { return };
        for payload in session.take_signals() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) {
                self.observe_hook(id, "osc", &value);
            }
        }
    }

    /// Bring a stopped terminal back in its own directory, and relay its
    /// signal. See `resume`.
    pub async fn resume_with_signal(
        &self,
        id: &str,
        rows: Option<u16>,
        cols: Option<u16>,
        on_activity: impl Fn(String) + Send + 'static,
    ) -> Result<HostedView, AppError> {
        let view = self.resume(id, rows, cols).await?;
        if let Ok(session) = self.get(id) {
            Self::relay(session, id.to_owned(), on_activity);
        } else if let Ok(Some(app)) = self.app(id) {
            Self::relay_app(app, id.to_owned(), on_activity);
        }
        Ok(view)
    }

    /// The structured transport's version of `relay`: wake the surface
    /// whenever the protocol said anything.
    pub(super) fn relay_app(
        app: Arc<appserver::CodexSession>,
        id: String,
        on_activity: impl Fn(String) + Send + 'static,
    ) {
        let changed = app.changed();
        tokio::spawn(async move {
            loop {
                let waiting = changed.notified();
                waiting.await;
                on_activity(id.clone());
                if !app.alive() {
                    break;
                }
            }
        });
    }

    /// Start a stopped terminal again where it was: the same worktree, the
    /// same arguments, plus whatever asks the tool to pick up its previous
    /// conversation there. The id, name, number and group are kept, so every
    /// tab and row that pointed at it still does.
    ///
    /// The output ring is the new process's; what the old one printed went
    /// with it. A directory that no longer exists is an error naming it,
    /// because starting a tool in a fresh empty directory of the same name
    /// would look like a resume and be nothing of the kind.
    pub async fn resume(
        &self,
        id: &str,
        rows: Option<u16>,
        cols: Option<u16>,
    ) -> Result<HostedView, AppError> {
        if let Some(app) = self.app(id)? {
            // A structured row resumes by starting its protocol again with
            // the thread it had; the next prompt picks the thread up.
            if app.alive() {
                return Err(AppError::Supervisor("still running".to_owned()));
            }
            return self.resume_app(id).await;
        }
        let (harness, place, args, provider) = {
            let held = self
                .sessions
                .lock()
                .expect("no lock is held across an await");
            let entry = held
                .get(id)
                .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?;
            if entry
                .live
                .as_ref()
                .is_some_and(|s| s.status() == HarnessStatus::Running)
            {
                return Err(AppError::Supervisor(format!(
                    "{} is still running",
                    view_of(id, entry).name
                )));
            }
            (
                entry.harness,
                entry.place.clone(),
                entry.args.clone(),
                entry.agent.session.clone(),
            )
        };
        if !place.cwd.is_dir() {
            return Err(AppError::Supervisor(format!(
                "its directory is gone: {}. Remove it from the list, or start a new one.",
                place.cwd.display()
            )));
        }
        let mut args = split_args(&args).map_err(AppError::Supervisor)?;
        let (hook_args, env) = self.hook_launch(harness, id);
        args.extend(hook_args);
        // Exact where the tool told us its session id; "the latest here"
        // where it did not. Codex's is read from the rollout it left in
        // this directory, which outranks whatever the journal holds — the
        // journal may carry the thread id its notify once named instead.
        let provider = match harness {
            Harness::Codex => super::agent::codex_rollout_for(&place.cwd)
                .map(|(session, _)| session)
                .or(provider),
            _ => provider,
        };
        args.extend(harness.resume_args_for(provider.as_deref()));
        let session = HarnessSession::spawn(harness, &place.cwd, &args, &env, rows, cols)
            .map_err(|e| AppError::Supervisor(e.to_string()))?;
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        let entry = held
            .get_mut(id)
            .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?;
        entry.live = Some(Arc::new(session));
        // Asked for again, so no longer stopped.
        entry.stopped = false;
        let view = view_of(id, entry);
        self.record(&held);
        Ok(view)
    }

    /// Stop one and keep its row: it is listed as ended, and can be resumed.
    /// Stopping what is not running is not an error — the row is already in
    /// the state asked for.
    pub async fn stop(&self, id: &str) -> Result<(), AppError> {
        let outcome = self.halt(id).await;
        // Remembered as a decision, not as an accident: this is a person
        // stopping an agent, and the next window must not undo that. A
        // window closing does not come through here — those processes die
        // with the process that owns them and their rows stay unmarked.
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        if let Some(entry) = held.get_mut(id) {
            entry.stopped = true;
        }
        self.record(&held);
        outcome
    }

    /// Stop the process behind a row and say nothing about why.
    async fn halt(&self, id: &str) -> Result<(), AppError> {
        if let Some(app) = self.app(id)? {
            app.close().await;
            return Ok(());
        }
        let live = {
            let held = self
                .sessions
                .lock()
                .expect("no lock is held across an await");
            held.get(id)
                .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?
                .live
                .clone()
        };
        match live {
            Some(session) => session
                .kill()
                .await
                .map_err(|e| AppError::Supervisor(e.to_string())),
            None => Ok(()),
        }
    }

    /// Stop one, and forget it — out of the list and out of the journal.
    ///
    /// Removed whatever the kill reports: a terminal somebody asked to remove
    /// must not stay in the list because stopping it was untidy. Its worktree
    /// is kept — what the agent did is a branch to read, land or delete.
    pub async fn kill(&self, id: &str) -> Result<(), AppError> {
        let outcome = self.halt(id).await;
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        held.remove(id);
        self.record(&held);
        outcome
    }

    /// Stop every live process and keep every row, for a window that is
    /// closing: the next one lists them all, ended.
    pub async fn stop_all(&self) {
        let ids: Vec<String> = self
            .sessions
            .lock()
            .expect("no lock is held across an await")
            .keys()
            .cloned()
            .collect();
        for id in ids {
            // `halt`, not `stop`: a window closing is not a person deciding
            // an agent should be stopped, and the next window brings these
            // back.
            let _ = self.halt(&id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row stopped by a person stays stopped across windows; one whose
    /// window merely closed does not.
    #[tokio::test]
    async fn stopping_is_remembered_and_closing_is_not() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let journal = dir.path().join("terminals.json");
        let row = |id: &str| {
            format!(
                r#"{{"id":"{id}","harness":"claude","cwd":"{d}","repo":"{d}","branch":null,"group":null,"title":null,"args":"","session":null,"transcript":null,"transport":"pty","stopped":false}}"#,
                d = dir.path().display()
            )
        };
        std::fs::write(&journal, format!("[{},{}]", row("h1"), row("h2"))).expect("written");

        let hosted = Hosted::remembered_in(journal.clone());
        assert!(hosted.list().iter().all(|v| !v.stopped));

        // Stopped by hand: marked here and in the journal.
        hosted
            .stop("h1")
            .await
            .expect("a row with no process is already stopped");
        let stopped = |h: &Hosted, id: &str| {
            h.list()
                .into_iter()
                .find(|v| v.id == id)
                .expect("listed")
                .stopped
        };
        assert!(stopped(&hosted, "h1"));
        assert!(!stopped(&hosted, "h2"));
        assert!(stopped(&Hosted::remembered_in(journal.clone()), "h1"));

        // A window closing stops every process and decides nothing.
        hosted.stop_all().await;
        assert!(!stopped(&hosted, "h2"));
        assert!(!stopped(&Hosted::remembered_in(journal), "h2"));
    }

    #[tokio::test]
    async fn acting_on_a_session_that_does_not_exist_is_an_error_not_a_panic() {
        let hosted = Hosted::default();
        assert!(hosted.read("nope", 0).is_err());
        assert!(hosted.write("nope", "hi", true).is_err());
        assert!(hosted.resize("nope", 24, 80).is_err());
        assert!(hosted.kill("nope").await.is_err());
        assert!(hosted.stop("nope").await.is_err());
        assert!(hosted.resume("nope", None, None).await.is_err());
        assert_eq!(hosted.running(), 0);
    }
}
