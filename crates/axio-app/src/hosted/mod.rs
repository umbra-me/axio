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
//!
//! The terminals outlive the window in the only sense a process can: what each
//! one *was* — its harness, its worktree, its branch, its name, its group — is
//! journaled to a file as it changes, and a window that opens later lists them
//! as ended, each one resume in place. A process cannot be kept across its
//! owner's exit; the work it was doing is a directory and a branch, and those
//! can. The resume asks the tool to continue its own conversation where the
//! tool has a way to be asked.

mod agent;
pub mod appserver;
mod commands;
mod journal;
mod lifecycle;
mod structured;
mod types;
mod typing;

pub use types::{HostedOutput, HostedView, Place, StartHostedInput};

pub use commands::SlashCommand;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axio_pty::{Harness, HarnessSession, HarnessStatus, split_args};

use crate::model::AppError;

/// One terminal and what the list knows about it that the process does not:
/// its number among its kind, where it was placed, any name a person gave
/// it, and the arguments it was started with — kept for a resume.
///
/// `live` is `None` for a terminal remembered from an earlier window, until it
/// is resumed. Everything else about it is still true.
struct Held {
    harness: Harness,
    live: Option<Arc<HarnessSession>>,
    /// The structured transport's process, for an `app` row; `live` is
    /// `None` on such a row.
    app: Option<Arc<appserver::CodexSession>>,
    transport: String,
    number: u32,
    place: Place,
    group: Option<String>,
    title: Option<String>,
    args: String,
    /// Stopped by a person. See `HostedView::stopped`.
    stopped: bool,
    /// What the agent has said about itself. See `agent.rs`.
    agent: agent::AgentState,
}

/// Every hosted terminal this process owns, and the ones it remembers.
///
/// Owned by Rust and only by Rust. A webview reload loses the interface, never
/// the terminals — which is the entire reason a reload can reattach by asking
/// for everything after a cursor rather than starting again.
#[derive(Default)]
pub struct Hosted {
    sessions: Mutex<BTreeMap<String, Held>>,
    next: Mutex<u64>,
    /// Where the list is written as it changes. `None` in tests and for a
    /// window with no home, and then nothing survives the process.
    journal: Option<PathBuf>,
    /// Where a hosted agent's hooks report, once the window is listening.
    /// `None` in tests and before the listener is up, and then tools get
    /// no hooks and status comes from their output alone.
    hooks: Mutex<Option<crate::hooks::HookEndpoint>>,
}

impl Hosted {
    fn mint(&self) -> String {
        let mut next = self.next.lock().expect("no lock is held across an await");
        *next += 1;
        format!("h{next}")
    }

    /// The lowest number no listed terminal of this harness holds. Numbers are
    /// reused so a third Claude Code opened after the first was removed is
    /// "2", not "3" — what the rail shows should count what is there.
    fn number_for(live: impl Iterator<Item = (Harness, u32)>, harness: Harness) -> u32 {
        let taken: Vec<u32> = live
            .filter(|(h, _)| *h == harness)
            .map(|(_, n)| n)
            .collect();
        (1..).find(|n| !taken.contains(n)).unwrap_or(1)
    }

    /// Start one, and relay its signal to `on_activity`.
    pub fn start_with_signal(
        &self,
        input: StartHostedInput,
        place: Place,
        on_activity: impl Fn(String) + Send + 'static,
    ) -> Result<HostedView, AppError> {
        let view = self.start_at(input, place)?;
        if let Ok(session) = self.get(&view.id) {
            Self::relay(session, view.id.clone(), on_activity);
        }
        Ok(view)
    }

    /// Start one in the directory the input names, directly. What a caller
    /// without a supervisor — a test, a state that could not open its index —
    /// gets; the state cuts a worktree first and calls `start_at`.
    pub fn start(&self, input: StartHostedInput) -> Result<HostedView, AppError> {
        let cwd = PathBuf::from(&input.cwd);
        let place = Place {
            repo: cwd.clone(),
            cwd,
            branch: None,
        };
        self.start_at(input, place)
    }

    /// Name one, or take the name away with `None`.
    pub fn rename(&self, id: &str, title: Option<String>) -> Result<HostedView, AppError> {
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        let entry = held
            .get_mut(id)
            .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?;
        entry.title = title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        let view = view_of(id, entry);
        self.record(&held);
        Ok(view)
    }

    /// What a hosted agent changed in its worktree, as a unified diff — the
    /// same view a session's `diff` gives, from the same code.
    pub async fn diff(&self, id: &str) -> Result<String, AppError> {
        let place = {
            let held = self
                .sessions
                .lock()
                .expect("no lock is held across an await");
            held.get(id)
                .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?
                .place
                .clone()
        };
        let isolation = if place.branch.is_some() {
            axio_supervisor::Isolation::Worktree
        } else {
            axio_supervisor::Isolation::Direct
        };
        axio_supervisor::Checkout {
            path: place.cwd,
            repo: place.repo,
            branch: place.branch,
            isolation,
        }
        .diff()
        .await
        .map_err(|e| AppError::Supervisor(e.to_string()))
    }

    pub fn start_at(&self, input: StartHostedInput, place: Place) -> Result<HostedView, AppError> {
        let harness = Harness::parse(&input.harness).ok_or_else(|| {
            AppError::NoRepository(format!("`{}` is not an agent axio can host", input.harness))
        })?;
        let mut args = split_args(&input.args).map_err(AppError::Supervisor)?;
        // The id first: the hooks carry it, so it must exist before spawn.
        let id = self.mint();
        let (hook_args, env) = self.hook_launch(harness, &id);
        args.extend(hook_args);
        let prompted = input.prompt.as_deref().filter(|p| !p.trim().is_empty());
        if let Some(prompt) = prompted
            && let Some(extra) = harness.prompt_args(prompt)
        {
            args.extend(extra);
        }

        let session =
            HarnessSession::spawn(harness, &place.cwd, &args, &env, input.rows, input.cols)
                .map_err(|e| AppError::Supervisor(e.to_string()))?;
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        let mut agent = agent::AgentState::default();
        if prompted.is_some() {
            agent.status = Some("working".to_owned());
        }
        let entry = Held {
            harness,
            live: Some(Arc::new(session)),
            app: None,
            transport: "pty".to_owned(),
            number: Self::number_for(held.values().map(|h| (h.harness, h.number)), harness),
            group: input.group.clone(),
            title: None,
            place,
            args: input.args.clone(),
            stopped: false,
            agent,
        };
        let view = view_of(&id, &entry);
        held.insert(id, entry);
        self.record(&held);
        Ok(view)
    }

    pub fn list(&self) -> Vec<HostedView> {
        self.sessions
            .lock()
            .expect("no lock is held across an await")
            .iter()
            .map(|(id, held)| view_of(id, held))
            .collect()
    }

    /// The structured process behind a row, when it is one.
    pub(super) fn app(&self, id: &str) -> Result<Option<Arc<appserver::CodexSession>>, AppError> {
        let held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        held.get(id)
            .map(|h| h.app.clone())
            .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))
    }

    /// The live process behind a row. A remembered row that has not been
    /// resumed has none, and says so rather than answering as if it had.
    pub(super) fn get(&self, id: &str) -> Result<Arc<HarnessSession>, AppError> {
        let held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        let entry = held
            .get(id)
            .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?;
        if entry.app.is_some() {
            return Err(AppError::Supervisor(format!(
                "{} is driven through its protocol, not a terminal",
                view_of(id, entry).name
            )));
        }
        entry.live.clone().ok_or_else(|| {
            AppError::Supervisor(format!(
                "{} is not running; resume it first",
                view_of(id, entry).name
            ))
        })
    }

    /// Everything after `from`. A read from the very start is a reattach —
    /// a fresh emulator catching up — and is stripped of the terminal
    /// queries the program asked on its way up, so the emulator does not
    /// answer them all over again into the program's stdin.
    pub fn read(&self, id: &str, from: u64) -> Result<HostedOutput, AppError> {
        self.observe_signals(id);
        let (bytes, cursor) = self.get(id)?.read_from(from);
        let bytes = if from == 0 {
            axio_pty::strip_queries(&bytes)
        } else {
            bytes
        };
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

    pub fn running(&self) -> usize {
        self.list().iter().filter(|v| v.status == "running").count()
    }
}

fn view_of(id: &str, held: &Held) -> HostedView {
    let (status, exit_code) = match (&held.app, held.live.as_ref().map(|s| s.status())) {
        (Some(app), _) => (if app.alive() { "running" } else { "ended" }, None),
        (None, Some(HarnessStatus::Running)) => ("running", None),
        (None, Some(HarnessStatus::Exited(code))) => ("exited", Some(code)),
        (None, Some(HarnessStatus::Ended) | None) => ("ended", None),
    };
    // A structured row's word is the protocol's; a terminal's is its hooks'.
    let (agent_status, provider_session) = match &held.app {
        Some(app) => (
            Some(app.status()),
            app.thread().or_else(|| held.agent.session.clone()),
        ),
        None => (held.agent.status.clone(), held.agent.session.clone()),
    };
    let label = held.harness.label();
    HostedView {
        id: id.to_owned(),
        harness: held.harness.executable().to_owned(),
        label: label.to_owned(),
        name: match &held.title {
            Some(title) => title.clone(),
            None if held.number == 1 => label.to_owned(),
            None => format!("{label} {}", held.number),
        },
        branch: held.place.branch.clone(),
        group: held.group.clone(),
        accent_var: held.harness.accent_var().to_owned(),
        cwd: held.place.cwd.display().to_string(),
        repo: held.place.repo.display().to_string(),
        status: status.to_owned(),
        exit_code,
        agent_status,
        provider_session,
        transport: held.transport.clone(),
        stopped: held.stopped,
    }
}

/// The agents this machine can host, for a picker.
///
/// Only those whose executable can be found. Offering one that cannot start
/// turns a launcher into an error message, and the message is a page of `PATH`.
pub fn available() -> Vec<HostedView> {
    Harness::ALL
        .iter()
        .filter(|harness| harness.locate().is_some())
        .map(|harness| HostedView {
            id: String::new(),
            harness: harness.executable().to_owned(),
            label: harness.label().to_owned(),
            name: harness.label().to_owned(),
            branch: None,
            group: None,
            accent_var: harness.accent_var().to_owned(),
            cwd: String::new(),
            repo: String::new(),
            status: "available".to_owned(),
            exit_code: None,
            agent_status: None,
            provider_session: None,
            transport: "pty".to_owned(),
            stopped: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which of the allowlist is offered depends on the machine; that every
    /// offer is on the allowlist and can actually be started does not.
    #[test]
    fn the_picker_offers_only_allowlisted_harnesses_that_can_be_found() {
        let offered = available();
        assert!(offered.len() <= Harness::ALL.len());
        for view in &offered {
            let harness = Harness::parse(&view.harness).expect("on the allowlist");
            assert!(
                harness.locate().is_some(),
                "{} was offered but cannot be found",
                view.label
            );
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
                isolation: None,
                group: None,
                args: String::new(),
                prompt: None,
                transport: None,
                model: None,
                effort: None,
                permission: None,
                rows: None,
                cols: None,
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
                    isolation: None,
                    group: None,
                    args: "--unbalanced \"quote".into(),
                    prompt: None,
                    transport: None,
                    model: None,
                    effort: None,
                    permission: None,
                    rows: None,
                    cols: None,
                })
                .is_err()
        );
        assert!(hosted.list().is_empty());
    }

    /// Numbers count what is listed, per harness, and fill the lowest gap.
    #[test]
    fn a_second_of_the_same_harness_is_numbered_and_a_gap_is_reused() {
        assert_eq!(Hosted::number_for(std::iter::empty(), Harness::Claude), 1);
        // Two Claude Codes numbered 1 and 3, and a Codex numbered 1: the
        // next Claude Code is 2, and Codex's numbering is its own.
        let live = [
            (Harness::Claude, 1),
            (Harness::Claude, 3),
            (Harness::Codex, 1),
        ];
        assert_eq!(Hosted::number_for(live.iter().copied(), Harness::Claude), 2);
        assert_eq!(Hosted::number_for(live.iter().copied(), Harness::Codex), 2);
        assert_eq!(Hosted::number_for(live.iter().copied(), Harness::Pi), 1);
    }
}
