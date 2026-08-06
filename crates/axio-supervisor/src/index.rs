//! The sidecar index.
//!
//! `SessionStore` derives a session's path from its own id, which makes
//! resuming free and listing a directory walk. That is the right trade for a
//! CLI showing the last few sessions; it is the wrong one for a surface that
//! asks "what is running on this project", because the answer is not in the
//! path and the header does not carry it — a worktree session's `cwd` is the
//! worktree, not the repository.
//!
//! So: an index, which the roadmap already named as the answer that comes
//! before a database. It is append-only and tolerant of its own damage, for the
//! same reasons the session file is: it is written while work is happening, and
//! a torn line must cost one entry rather than the file.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use axio_core::protocol::SessionId;
use serde::{Deserialize, Serialize};

use crate::error::{Result, SupervisorError};
use crate::project::{Project, ProjectId, Projects};
use crate::worktree::{Checkout, Isolation};

/// What the index knows about one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub session: SessionId,
    pub project: ProjectId,
    pub project_root: PathBuf,
    pub project_name: String,
    /// Where the agent worked. The repository root under [`Isolation::Direct`].
    pub workspace: PathBuf,
    pub branch: Option<String>,
    pub isolation: Isolation,
    pub label: Option<String>,
    pub started_ms: u64,
    /// Set when the session was closed. An open session is one with no closing
    /// record, which is also how a crashed one looks — correctly, since its
    /// worktree is still there and still holds work.
    #[serde(default)]
    pub closed_ms: Option<u64>,
    #[serde(default)]
    pub discarded: bool,
}

impl IndexEntry {
    pub fn is_open(&self) -> bool {
        self.closed_ms.is_none()
    }

    pub fn project(&self) -> Project {
        Project {
            id: self.project.clone(),
            root: self.project_root.clone(),
            name: self.project_name.clone(),
        }
    }

    pub fn checkout(&self) -> Checkout {
        Checkout {
            path: self.workspace.clone(),
            repo: self.project_root.clone(),
            branch: self.branch.clone(),
            isolation: self.isolation,
        }
    }
}

/// One line of the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "rec", rename_all = "snake_case")]
enum IndexRecord {
    Started {
        #[serde(flatten)]
        entry: Box<IndexEntry>,
    },
    Closed {
        session: SessionId,
        at_ms: u64,
        discarded: bool,
    },
    /// Written by a later axio. Skipped, never fatal — the same forward
    /// compatibility the session format has.
    #[serde(other)]
    Unknown,
}

/// Every session the supervisor has started, and what became of it.
///
/// Held in **file order**, not sorted by id. A ULID orders by time only across
/// milliseconds: inside one, the remaining bits are random, so two sessions
/// started together sort arbitrarily — and a queue of agents starts sessions
/// together as a matter of course. The append-only file already records the
/// order things happened, which is the answer that cannot tie.
#[derive(Debug)]
pub struct SessionIndex {
    path: PathBuf,
    entries: Vec<IndexEntry>,
    at: BTreeMap<SessionId, usize>,
}

impl SessionIndex {
    /// Read the index, or start one.
    ///
    /// A line that will not parse is skipped rather than fatal. The index is
    /// derived state — worst case it under-reports, and a supervisor that
    /// refuses to start because its own cache is torn is a worse outcome than
    /// one that shows a session late.
    pub fn open(path: PathBuf) -> Result<Self> {
        let mut index = Self {
            path,
            entries: Vec::new(),
            at: BTreeMap::new(),
        };

        match std::fs::File::open(&index.path) {
            Ok(file) => {
                for line in BufReader::new(file)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<IndexRecord>(trimmed) {
                        Ok(IndexRecord::Started { entry }) => index.remember(*entry),
                        Ok(IndexRecord::Closed {
                            session,
                            at_ms,
                            discarded,
                        }) => index.mark_closed(session, at_ms, discarded),
                        Ok(IndexRecord::Unknown) => {}
                        Err(e) => tracing::warn!("skipping a damaged index line: {e}"),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(SupervisorError::io(
                    format!("reading {}", index.path.display()),
                    e,
                ));
            }
        }

        Ok(index)
    }

    /// Keep file order, and let a repeated `started` correct its predecessor
    /// rather than appear twice.
    fn remember(&mut self, entry: IndexEntry) {
        match self.at.get(&entry.session) {
            Some(&position) => self.entries[position] = entry,
            None => {
                self.at.insert(entry.session, self.entries.len());
                self.entries.push(entry);
            }
        }
    }

    fn mark_closed(&mut self, session: SessionId, at_ms: u64, discarded: bool) {
        if let Some(&position) = self.at.get(&session)
            && let Some(entry) = self.entries.get_mut(position)
        {
            entry.closed_ms = Some(at_ms);
            entry.discarded = discarded;
        }
    }

    fn append(&self, record: &IndexRecord) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SupervisorError::io(format!("creating {}", parent.display()), e))?;
        }
        let mut line = serde_json::to_string(record).map_err(|e| {
            SupervisorError::io(
                "serialising an index record",
                std::io::Error::other(e.to_string()),
            )
        })?;
        line.push('\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| SupervisorError::io(format!("opening {}", self.path.display()), e))?;
        file.write_all(line.as_bytes())
            .map_err(|e| SupervisorError::io(format!("writing {}", self.path.display()), e))?;
        Ok(())
    }

    pub fn record_started(&mut self, entry: IndexEntry) -> Result<()> {
        self.append(&IndexRecord::Started {
            entry: Box::new(entry.clone()),
        })?;
        self.remember(entry);
        Ok(())
    }

    pub fn record_closed(&mut self, session: SessionId, discarded: bool) -> Result<()> {
        let at_ms = crate::approval::now_ms();
        self.append(&IndexRecord::Closed {
            session,
            at_ms,
            discarded,
        })?;
        self.mark_closed(session, at_ms, discarded);
        Ok(())
    }

    pub fn get(&self, session: SessionId) -> Option<&IndexEntry> {
        self.at.get(&session).and_then(|&at| self.entries.get(at))
    }

    /// Every session, newest first — the order they were written in, reversed.
    pub fn all(&self) -> Vec<IndexEntry> {
        self.entries.iter().rev().cloned().collect()
    }

    pub fn for_project(&self, project: &ProjectId) -> Vec<IndexEntry> {
        self.all()
            .into_iter()
            .filter(|e| &e.project == project)
            .collect()
    }

    /// The projects that have work, rebuilt from the sessions themselves.
    ///
    /// This is why there is no separate project registry on disk: a second list
    /// could disagree with this one, and there would be no way to say which was
    /// right.
    pub fn projects(&self) -> Projects {
        let mut projects = Projects::default();
        for entry in &self.entries {
            projects.insert(entry.project());
        }
        projects
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(project: &Project, label: &str) -> IndexEntry {
        IndexEntry {
            session: SessionId::generate(),
            project: project.id.clone(),
            project_root: project.root.clone(),
            project_name: project.name.clone(),
            workspace: project.root.join("wt"),
            branch: Some("axio/x".into()),
            isolation: Isolation::Worktree,
            label: Some(label.to_owned()),
            started_ms: 1,
            closed_ms: None,
            discarded: false,
        }
    }

    fn project(name: &str) -> Project {
        let root = PathBuf::from(format!("/repos/{name}"));
        Project {
            id: ProjectId::of(&root),
            root,
            name: name.to_owned(),
        }
    }

    #[test]
    fn an_index_survives_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.jsonl");
        let one = project("one");
        let session;

        {
            let mut index = SessionIndex::open(path.clone()).unwrap();
            let e = entry(&one, "do a thing");
            session = e.session;
            index.record_started(e).unwrap();
        }

        let index = SessionIndex::open(path).unwrap();
        let found = index.get(session).expect("the session survived");
        assert_eq!(found.label.as_deref(), Some("do a thing"));
        assert!(found.is_open());
    }

    #[test]
    fn closing_is_a_record_rather_than_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.jsonl");
        let one = project("one");
        let mut index = SessionIndex::open(path.clone()).unwrap();
        let e = entry(&one, "x");
        let session = e.session;
        index.record_started(e).unwrap();
        index.record_closed(session, true).unwrap();

        // Re-read: the fold has to produce the same answer as the live map.
        let reopened = SessionIndex::open(path).unwrap();
        let found = reopened.get(session).unwrap();
        assert!(!found.is_open());
        assert!(found.discarded);
    }

    #[test]
    fn sessions_are_grouped_by_project_and_listed_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SessionIndex::open(dir.path().join("index.jsonl")).unwrap();
        let (one, two) = (project("one"), project("two"));
        let first = entry(&one, "first");
        let second = entry(&one, "second");
        index.record_started(first.clone()).unwrap();
        index.record_started(second.clone()).unwrap();
        index.record_started(entry(&two, "elsewhere")).unwrap();

        let mine = index.for_project(&one.id);
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].session, second.session, "newest first");
        assert_eq!(index.all().len(), 3);
    }

    #[test]
    fn the_project_list_is_rebuilt_from_the_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SessionIndex::open(dir.path().join("index.jsonl")).unwrap();
        let (one, two) = (project("one"), project("two"));
        index.record_started(entry(&one, "a")).unwrap();
        index.record_started(entry(&one, "b")).unwrap();
        index.record_started(entry(&two, "c")).unwrap();

        let names: Vec<String> = index.projects().all().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["one", "two"], "one project per repository");
    }

    /// The index is derived state. A torn line costs one entry; refusing to
    /// start because a cache is damaged is the worse failure.
    #[test]
    fn a_damaged_line_costs_one_entry_not_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.jsonl");
        let one = project("one");
        let good;
        {
            let mut index = SessionIndex::open(path.clone()).unwrap();
            let e = entry(&one, "kept");
            good = e.session;
            index.record_started(e).unwrap();
        }
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"rec\":\"started\",\"session\":\"tru\n");
        std::fs::write(&path, text).unwrap();

        let index = SessionIndex::open(path).unwrap();
        assert_eq!(index.all().len(), 1);
        assert!(index.get(good).is_some());
    }

    #[test]
    fn a_record_kind_from_a_later_version_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.jsonl");
        std::fs::write(&path, "{\"rec\":\"invented_later\",\"x\":1}\n").unwrap();
        assert!(SessionIndex::open(path).unwrap().all().is_empty());
    }
}
