//! Other agents' command-line tools, hosted in terminals axio owns.
//!
//! A supervised session is axio driving its own loop; a hosted one is Claude
//! Code, Codex or Pi running as itself, with its own interface and its own
//! prompts, in a pseudo-terminal this process holds. They sit side by side
//! deliberately — the point of the window is every agent working on your code,
//! not only the one that happens to be ours.
//!
//! What is *not* here is any attempt to parse them. A hosted agent's output is
//! bytes on their way to a terminal emulator; interpreting it to guess what the
//! agent is doing would be a second, worse implementation of the thing it
//! already does correctly on screen.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axio_pty::{Harness, HarnessSession, HarnessStatus, split_args};
use serde::{Deserialize, Serialize};

use crate::model::AppError;

/// A hosted agent, as a list row sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedView {
    pub id: String,
    pub harness: String,
    pub label: String,
    /// The CSS custom property this harness is coloured with. Decided in Rust
    /// beside the harness list, so a colour and the thing it identifies cannot
    /// be two lists that disagree.
    pub accent_var: String,
    pub cwd: String,
    pub status: String,
    /// Set only once it has stopped.
    pub exit_code: Option<i32>,
}

/// What starting one takes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartHostedInput {
    /// One of the allowlisted names. Never a command line.
    pub harness: String,
    pub cwd: String,
    /// Extra arguments, split the way a shell would split them without one
    /// running. Empty is the normal case.
    #[serde(default)]
    pub args: String,
}

/// Everything a read returns: the bytes, and where to ask from next.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedOutput {
    /// Decoded here rather than in the webview, because this side holds the
    /// whole stream and can decode across the chunk boundaries a `read` lands
    /// on. Lossy only at the very end, where a trailing partial character is
    /// genuinely incomplete rather than merely split.
    pub text: String,
    pub cursor: u64,
}

/// Every hosted terminal this process owns.
///
/// Owned by Rust and only by Rust. A webview reload loses the interface, never
/// the terminals — which is the entire reason a reload can reattach by asking
/// for everything after a cursor rather than starting again.
#[derive(Default)]
pub struct Hosted {
    sessions: Mutex<BTreeMap<String, Arc<HarnessSession>>>,
    next: Mutex<u64>,
}

impl Hosted {
    fn mint(&self) -> String {
        let mut next = self.next.lock().expect("no lock is held across an await");
        *next += 1;
        format!("h{next}")
    }

    pub fn start(&self, input: StartHostedInput) -> Result<HostedView, AppError> {
        let harness = Harness::parse(&input.harness).ok_or_else(|| {
            AppError::NoRepository(format!("`{}` is not an agent axio can host", input.harness))
        })?;
        let args = split_args(&input.args).map_err(AppError::Supervisor)?;
        let cwd = std::path::PathBuf::from(&input.cwd);

        let session = HarnessSession::spawn(harness, &cwd, &args)
            .map_err(|e| AppError::Supervisor(e.to_string()))?;
        let id = self.mint();
        let view = view_of(&id, &session);
        self.sessions
            .lock()
            .expect("no lock is held across an await")
            .insert(id, Arc::new(session));
        Ok(view)
    }

    pub fn list(&self) -> Vec<HostedView> {
        self.sessions
            .lock()
            .expect("no lock is held across an await")
            .iter()
            .map(|(id, session)| view_of(id, session))
            .collect()
    }

    fn get(&self, id: &str) -> Result<Arc<HarnessSession>, AppError> {
        self.sessions
            .lock()
            .expect("no lock is held across an await")
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))
    }

    pub fn read(&self, id: &str, from: u64) -> Result<HostedOutput, AppError> {
        let (bytes, cursor) = self.get(id)?.read_from(from);
        Ok(HostedOutput {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            cursor,
        })
    }

    /// Send keystrokes.
    ///
    /// A submitted line arrives as two writes — the text, then the carriage
    /// return — because a provider that treats one combined chunk as a paste
    /// leaves the text on its prompt unsent, which reads as the agent ignoring
    /// you.
    pub fn write(&self, id: &str, data: &str, submit: bool) -> Result<(), AppError> {
        let session = self.get(id)?;
        session
            .write(data.as_bytes())
            .map_err(|e| AppError::Supervisor(e.to_string()))?;
        if submit {
            session
                .write(b"\r")
                .map_err(|e| AppError::Supervisor(e.to_string()))?;
        }
        Ok(())
    }

    pub fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), AppError> {
        self.get(id)?
            .resize(rows, cols)
            .map_err(|e| AppError::Supervisor(e.to_string()))
    }

    /// Stop one, and forget it.
    ///
    /// Removed from the map whatever the kill reports: a terminal somebody
    /// asked to close must not stay in the list because stopping it was untidy.
    pub async fn kill(&self, id: &str) -> Result<(), AppError> {
        let session = self.get(id)?;
        let outcome = session.kill().await;
        self.sessions
            .lock()
            .expect("no lock is held across an await")
            .remove(id);
        outcome.map_err(|e| AppError::Supervisor(e.to_string()))
    }

    /// Stop everything, for a window that is closing.
    pub async fn kill_all(&self) {
        let ids: Vec<String> = self
            .sessions
            .lock()
            .expect("no lock is held across an await")
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let _ = self.kill(&id).await;
        }
    }

    pub fn running(&self) -> usize {
        self.list().iter().filter(|v| v.status == "running").count()
    }
}

fn view_of(id: &str, session: &HarnessSession) -> HostedView {
    let (status, exit_code) = match session.status() {
        HarnessStatus::Running => ("running", None),
        HarnessStatus::Exited(code) => ("exited", Some(code)),
        HarnessStatus::Ended => ("ended", None),
    };
    HostedView {
        id: id.to_owned(),
        harness: session.harness.executable().to_owned(),
        label: session.harness.label().to_owned(),
        accent_var: session.harness.accent_var().to_owned(),
        cwd: session.cwd.display().to_string(),
        status: status.to_owned(),
        exit_code,
    }
}

/// The agents this build can host, for a picker.
pub fn available() -> Vec<HostedView> {
    Harness::ALL
        .iter()
        .map(|harness| HostedView {
            id: String::new(),
            harness: harness.executable().to_owned(),
            label: harness.label().to_owned(),
            accent_var: harness.accent_var().to_owned(),
            cwd: String::new(),
            status: "available".to_owned(),
            exit_code: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picker_offers_exactly_the_allowlist() {
        let offered = available();
        assert_eq!(offered.len(), Harness::ALL.len());
        for view in &offered {
            assert!(Harness::parse(&view.harness).is_some());
            assert!(view.accent_var.starts_with("--agent-"));
        }
    }

    #[test]
    fn a_name_outside_the_allowlist_is_refused_before_anything_spawns() {
        let hosted = Hosted::default();
        let err = hosted
            .start(StartHostedInput {
                harness: "rm".into(),
                cwd: ".".into(),
                args: String::new(),
            })
            .expect_err("the allowlist is the security property");
        assert!(err.to_string().contains("rm"));
        assert!(hosted.list().is_empty(), "nothing was started");
    }

    #[test]
    fn arguments_that_cannot_be_split_are_refused_before_anything_spawns() {
        let hosted = Hosted::default();
        assert!(
            hosted
                .start(StartHostedInput {
                    harness: "claude".into(),
                    cwd: ".".into(),
                    args: "--unbalanced \"quote".into(),
                })
                .is_err()
        );
        assert!(hosted.list().is_empty());
    }

    #[tokio::test]
    async fn acting_on_a_session_that_does_not_exist_is_an_error_not_a_panic() {
        let hosted = Hosted::default();
        assert!(hosted.read("nope", 0).is_err());
        assert!(hosted.write("nope", "hi", true).is_err());
        assert!(hosted.resize("nope", 24, 80).is_err());
        assert!(hosted.kill("nope").await.is_err());
        assert_eq!(hosted.running(), 0);
    }
}
