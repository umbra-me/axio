//! Sessions running beside the one you are typing into.
//!
//! `/new` hands a prompt to the supervisor and returns immediately. The work
//! happens in its own git worktree on its own branch, so it cannot touch the
//! checkout this session is in, and several can be in flight at once.
//!
//! Progress comes back as notes rather than as the raw event stream. The
//! viewport belongs to the session being typed into; a background turn that
//! streamed its tokens into it would make the foreground unreadable exactly
//! when there is most to read. What lands in scrollback is what a person needs
//! to decide something: it started, it finished, this much changed, here is how
//! to look at it.

use std::sync::Arc;

use axio_core::protocol::SessionId;
use axio_supervisor::{Disposition, StartOptions, Supervisor};
use tokio::sync::mpsc;

/// What a background session tells the surface.
#[derive(Debug)]
pub enum Note {
    Started {
        session: SessionId,
        branch: Option<String>,
    },
    Ended {
        session: SessionId,
        outcome: String,
        changed: usize,
    },
    /// Anything that went wrong before or instead of a turn. Carries its own
    /// prose because the causes are unrelated — no repository, no commits to
    /// branch from, a provider that cannot start — and a single wording for all
    /// three would describe none of them.
    Failed { message: String },
}

impl Note {
    /// One line per note, as it appears in scrollback.
    pub fn lines(&self) -> Vec<String> {
        match self {
            Note::Started { session, branch } => {
                let mut out = vec![format!("started {}", short(*session))];
                if let Some(branch) = branch {
                    out.push(format!("  branch  {branch}"));
                }
                out
            }
            Note::Ended {
                session,
                outcome,
                changed,
            } => vec![
                format!("{} {}", short(*session), outcome),
                match changed {
                    0 => "  changed nothing".to_owned(),
                    1 => "  1 file changed".to_owned(),
                    n => format!("  {n} files changed"),
                },
                format!("  axio session diff {}", short(*session)),
            ],
            Note::Failed { message } => vec![format!("could not start: {message}")],
        }
    }
}

/// Start a session in its own worktree and run one turn in it.
///
/// Spawned rather than awaited: the whole point is that the composer stays
/// live. Every exit path sends a note, including the ones that fail before a
/// session exists, because a `/new` that reports nothing is indistinguishable
/// from one that was never read.
pub fn spawn(
    supervisor: Arc<Supervisor>,
    prompt: String,
    notes: mpsc::UnboundedSender<Note>,
    repo: std::path::PathBuf,
) {
    tokio::spawn(async move {
        let handle = match supervisor
            .start(
                &repo,
                StartOptions {
                    label: Some(prompt.clone()),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(handle) => handle,
            Err(e) => {
                let _ = notes.send(Note::Failed {
                    message: e.to_string(),
                });
                return;
            }
        };

        let _ = notes.send(Note::Started {
            session: handle.session,
            branch: handle.checkout.branch.clone(),
        });

        let outcome = match handle.turn(prompt).await {
            Ok(outcome) => outcome.summary(),
            Err(e) => {
                let _ = notes.send(Note::Failed {
                    message: e.to_string(),
                });
                return;
            }
        };

        let changed = handle.checkout.status().await.map(|s| s.len()).unwrap_or(0);
        // Kept, always. The branch is the work, and a background turn ending is
        // not a decision to throw it away — that is what `axio session close`
        // is for, once someone has looked.
        let _ = supervisor.close(handle.session, Disposition::Keep).await;

        let _ = notes.send(Note::Ended {
            session: handle.session,
            outcome,
            changed,
        });
    });
}

/// What `/sessions` prints.
///
/// Reads the index rather than only what is live, so a session from an earlier
/// run is listed too. A list that forgot everything when the process restarted
/// would be a list of this window rather than of the work.
pub fn summary(supervisor: &Supervisor) -> Vec<String> {
    let history = supervisor.history();
    if history.is_empty() {
        return vec!["no sessions yet — /new <prompt> starts one".to_owned()];
    }

    let live: usize = supervisor.sessions().len();
    let mut out = vec![format!("{} session(s), {live} running", history.len())];
    let mut project = String::new();
    for entry in history.iter().take(12) {
        if entry.project_name != project {
            project.clone_from(&entry.project_name);
            out.push(format!("  {project}"));
        }
        out.push(format!(
            "    {}  {:<7}  {}",
            short(entry.session),
            if entry.is_open() { "open" } else { "closed" },
            entry.label.as_deref().unwrap_or("(no prompt)")
        ));
    }
    if history.len() > 12 {
        out.push(format!("  … and {} more", history.len() - 12));
    }
    out
}

fn short(id: SessionId) -> String {
    id.to_string().chars().take(8).collect()
}
