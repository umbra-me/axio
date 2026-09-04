//! What the next window is told about this one's terminals.
//!
//! A process cannot be kept across its owner's exit; what each terminal *was*
//! can. The list is written whole on every change and read once at start.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use axio_pty::Harness;
use serde::{Deserialize, Serialize};

use super::{Held, Hosted, Place};

/// One row of the journal: a terminal as the next window will need it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Remembered {
    id: String,
    harness: Harness,
    cwd: PathBuf,
    repo: PathBuf,
    branch: Option<String>,
    group: Option<String>,
    title: Option<String>,
    #[serde(default)]
    args: String,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    transcript: Option<PathBuf>,
    #[serde(default = "pty")]
    transport: String,
    /// Stopped by a person, so the next window leaves it alone.
    #[serde(default)]
    stopped: bool,
}

fn pty() -> String {
    "pty".to_owned()
}

impl Hosted {
    /// A registry that remembers its terminals in `journal`, starting from
    /// whatever a previous window left there.
    ///
    /// A journal that cannot be read is treated as empty and said so on
    /// stderr, rather than refusing to open a window over one bad file: the
    /// rows are a convenience, and the next write replaces the file.
    pub fn remembered_in(journal: PathBuf) -> Self {
        let mut held = BTreeMap::new();
        let mut highest = 0u64;
        match std::fs::read_to_string(&journal) {
            Ok(text) => match serde_json::from_str::<Vec<Remembered>>(&text) {
                Ok(rows) => {
                    for row in rows {
                        if let Some(n) = row.id.strip_prefix('h').and_then(|n| n.parse().ok()) {
                            highest = highest.max(n);
                        }
                        let number = Self::number_for(
                            held.values().map(|h: &Held| (h.harness, h.number)),
                            row.harness,
                        );
                        held.insert(
                            row.id,
                            Held {
                                harness: row.harness,
                                live: None,
                                app: None,
                                transport: row.transport,
                                stopped: row.stopped,
                                number,
                                place: Place {
                                    cwd: row.cwd,
                                    repo: row.repo,
                                    branch: row.branch,
                                },
                                group: row.group,
                                title: row.title,
                                args: row.args,
                                agent: super::agent::AgentState {
                                    status: None,
                                    session: row.session,
                                    transcript: row.transcript,
                                },
                            },
                        );
                    }
                }
                Err(e) => eprintln!(
                    "axio-app: {} is not a terminal list: {e}",
                    journal.display()
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("axio-app: {} could not be read: {e}", journal.display()),
        }
        Self {
            sessions: Mutex::new(held),
            next: Mutex::new(highest),
            journal: Some(journal),
            hooks: Mutex::new(None),
        }
    }

    /// Write the list, whole, to the journal. Called with the map's lock
    /// already held, so a write always reflects one consistent state.
    ///
    /// Written beside and renamed over: a window killed mid-write leaves the
    /// previous list, never half of this one. A failure is said on stderr and
    /// otherwise ignored — the terminal has already started, and refusing to
    /// list it because a file could not be written helps nobody.
    pub(super) fn record(&self, held: &BTreeMap<String, Held>) {
        let Some(path) = &self.journal else {
            return;
        };
        let rows: Vec<Remembered> = held
            .iter()
            .map(|(id, h)| Remembered {
                id: id.clone(),
                harness: h.harness,
                cwd: h.place.cwd.clone(),
                repo: h.place.repo.clone(),
                branch: h.place.branch.clone(),
                group: h.group.clone(),
                title: h.title.clone(),
                args: h.args.clone(),
                session: h.agent.session.clone(),
                transcript: h.agent.transcript.clone(),
                transport: h.transport.clone(),
                stopped: h.stopped,
            })
            .collect();
        let written = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let text = serde_json::to_string_pretty(&rows)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, path)
        })();
        if let Err(e) = written {
            eprintln!(
                "axio-app: the terminal list could not be written to {}: {e}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A journal written by one window is the list the next one opens with:
    /// every row ended, named and numbered as it was, and none of them
    /// answerable until resumed.
    #[tokio::test]
    async fn a_remembered_terminal_is_listed_ended_and_forgotten_on_kill() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let journal = dir.path().join("terminals.json");
        let rows = vec![
            Remembered {
                id: "h3".into(),
                harness: Harness::Claude,
                cwd: dir.path().to_path_buf(),
                repo: dir.path().to_path_buf(),
                branch: Some("axio/claude-1".into()),
                group: None,
                title: Some("the refactor".into()),
                args: "--model opus".into(),
                session: Some("s-old".into()),
                transcript: None,
                transport: "pty".into(),
                stopped: false,
            },
            Remembered {
                id: "h7".into(),
                harness: Harness::Claude,
                cwd: dir.path().join("gone"),
                repo: dir.path().to_path_buf(),
                branch: None,
                group: Some("g1".into()),
                title: None,
                args: String::new(),
                session: None,
                transcript: None,
                transport: "pty".into(),
                stopped: true,
            },
        ];
        std::fs::write(&journal, serde_json::to_string(&rows).unwrap()).unwrap();

        let hosted = Hosted::remembered_in(journal.clone());
        let list = hosted.list();
        assert_eq!(list.len(), 2);
        let first = list.iter().find(|v| v.id == "h3").expect("h3 is listed");
        assert_eq!(first.status, "ended");
        assert_eq!(first.name, "the refactor");
        assert_eq!(first.branch.as_deref(), Some("axio/claude-1"));
        let second = list.iter().find(|v| v.id == "h7").expect("h7 is listed");
        assert_eq!(
            second.name, "Claude Code 2",
            "numbered among what is listed"
        );
        assert_eq!(second.group.as_deref(), Some("g1"));
        assert_eq!(hosted.running(), 0);

        // Not running: the terminal cannot be read or typed at.
        assert!(hosted.read("h3", 0).is_err());
        assert!(hosted.write("h3", "hi", false).is_err());
        // A directory that is gone is refused by name, not started fresh.
        let err = hosted
            .resume("h7", None, None)
            .await
            .expect_err("the directory is gone");
        assert!(err.to_string().contains("gone"), "{err}");

        // The next id does not collide with a remembered one.
        assert_eq!(hosted.mint(), "h8");

        // Forgetting removes the row from the journal; the other stays.
        hosted
            .kill("h3")
            .await
            .expect("a remembered row can be forgotten");
        let again = Hosted::remembered_in(journal);
        let ids: Vec<String> = again.list().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, vec!["h7".to_owned()]);
    }

    /// A journal that is not a list is an empty list, said on stderr, not a
    /// window that will not open.
    #[tokio::test]
    async fn a_broken_journal_is_an_empty_list() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let journal = dir.path().join("terminals.json");
        std::fs::write(&journal, "{not json").unwrap();
        let hosted = Hosted::remembered_in(journal);
        assert!(hosted.list().is_empty());
        assert!(hosted.kill("h1").await.is_err());
    }
}
