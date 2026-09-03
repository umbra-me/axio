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

mod typing;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axio_pty::{Harness, HarnessSession, HarnessStatus, split_args};
use serde::{Deserialize, Serialize};

use crate::model::{AppError, Isolation};

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
    pub status: String,
    /// Set only once it has stopped.
    pub exit_code: Option<i32>,
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

/// One live terminal and what the list knows about it that the session does
/// not: its number among its kind, where it was placed, and any name a
/// person gave it.
struct Held {
    session: Arc<HarnessSession>,
    number: u32,
    place: Place,
    group: Option<String>,
    title: Option<String>,
}

/// Every hosted terminal this process owns.
///
/// Owned by Rust and only by Rust. A webview reload loses the interface, never
/// the terminals — which is the entire reason a reload can reattach by asking
/// for everything after a cursor rather than starting again.
#[derive(Default)]
pub struct Hosted {
    sessions: Mutex<BTreeMap<String, Held>>,
    next: Mutex<u64>,
}

impl Hosted {
    fn mint(&self) -> String {
        let mut next = self.next.lock().expect("no lock is held across an await");
        *next += 1;
        format!("h{next}")
    }

    /// The lowest number no live terminal of this harness holds. Numbers are
    /// reused so a third Claude Code opened after the first closed is "2", not
    /// "3" — what the rail shows should count what is there.
    fn number_for(live: impl Iterator<Item = (Harness, u32)>, harness: Harness) -> u32 {
        let taken: Vec<u32> = live
            .filter(|(h, _)| *h == harness)
            .map(|(_, n)| n)
            .collect();
        (1..).find(|n| !taken.contains(n)).unwrap_or(1)
    }

    /// Start one, and relay its "something happened" signal to `on_activity`.
    ///
    /// The callback is given the session id and nothing else, deliberately.
    /// What it is for is telling a surface to *ask*, not telling it what
    /// changed — the bytes still come back through a cursor, so a listener that
    /// missed a signal is late rather than wrong.
    pub fn start_with_signal(
        &self,
        input: StartHostedInput,
        place: Place,
        on_activity: impl Fn(String) + Send + 'static,
    ) -> Result<HostedView, AppError> {
        let view = self.start_at(input, place)?;
        let id = view.id.clone();
        if let Ok(session) = self.get(&id) {
            let wrote = session.wrote();
            tokio::spawn(async move {
                loop {
                    // Registered before the wait, which is the whole discipline
                    // of `Notify`: created afterwards, output landing in the gap
                    // would wake nobody.
                    let waiting = wrote.notified();
                    waiting.await;
                    on_activity(id.clone());
                }
            });
        }
        Ok(view)
    }

    /// Start one in the directory the input names, directly. What a caller
    /// without a supervisor — a test, a state that could not open its index —
    /// gets; the state cuts a worktree first and calls `start_at`.
    pub fn start(&self, input: StartHostedInput) -> Result<HostedView, AppError> {
        let cwd = std::path::PathBuf::from(&input.cwd);
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
        Ok(view_of(id, entry))
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
        let args = split_args(&input.args).map_err(AppError::Supervisor)?;

        let session = HarnessSession::spawn(harness, &place.cwd, &args, input.rows, input.cols)
            .map_err(|e| AppError::Supervisor(e.to_string()))?;
        let id = self.mint();
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        let entry = Held {
            session: Arc::new(session),
            number: Self::number_for(
                held.values().map(|h| (h.session.harness, h.number)),
                harness,
            ),
            group: input.group.clone(),
            title: None,
            place,
        };
        let view = view_of(&id, &entry);
        held.insert(id, entry);
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

    pub(super) fn get(&self, id: &str) -> Result<Arc<HarnessSession>, AppError> {
        self.sessions
            .lock()
            .expect("no lock is held across an await")
            .get(id)
            .map(|h| h.session.clone())
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

fn view_of(id: &str, held: &Held) -> HostedView {
    let session = &held.session;
    let (status, exit_code) = match session.status() {
        HarnessStatus::Running => ("running", None),
        HarnessStatus::Exited(code) => ("exited", Some(code)),
        HarnessStatus::Ended => ("ended", None),
    };
    let label = session.harness.label();
    HostedView {
        id: id.to_owned(),
        harness: session.harness.executable().to_owned(),
        label: label.to_owned(),
        name: match &held.title {
            Some(title) => title.clone(),
            None if held.number == 1 => label.to_owned(),
            None => format!("{label} {}", held.number),
        },
        branch: held.place.branch.clone(),
        group: held.group.clone(),
        accent_var: session.harness.accent_var().to_owned(),
        cwd: session.cwd.display().to_string(),
        status: status.to_owned(),
        exit_code,
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
            status: "available".to_owned(),
            exit_code: None,
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
                    rows: None,
                    cols: None,
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

    /// Numbers count what is live, per harness, and fill the lowest gap.
    #[test]
    fn a_second_of_the_same_harness_is_numbered_and_a_gap_is_reused() {
        assert_eq!(Hosted::number_for(std::iter::empty(), Harness::Claude), 1);
        // Two live Claude Codes numbered 1 and 3, and a Codex numbered 1: the
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
