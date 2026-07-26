//! The session file: an append-only JSONL log.
//!
//! One rule decides everything else here: **the file is the record of what
//! happened, not of what was sent.** Compaction is a request-shaping decision
//! made per step from the transcript in memory, and it never reaches disk. So a
//! resumed session rebuilds the full history and re-derives its elisions, which
//! is what makes a resume reproducible rather than a slow drift.
//!
//! Line one is always the header, written by the same call that creates the
//! file. `--list` reads exactly that line and never parses a transcript, which
//! is what keeps listing cheap as sessions accumulate.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::protocol::{Item, ItemBody, Notice, SessionId, ToolStatus, TurnOutcome, Usage};
use crate::session::Session;

/// The container format. An unknown value is a refusal to load, not a parse
/// error two thousand lines in.
pub const SESSION_FORMAT_VERSION: u32 = 1;

/// Never read more than this looking for a header. A corrupt file whose first
/// byte is not a newline must not make `--list` allocate the whole thing.
const HEADER_MAX_BYTES: u64 = 64 * 1024;

/// Creation metadata. Everything here is knowable when the file is created and
/// never changes — which is the constraint an append-only first line imposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    pub protocol: u32,
    pub id: SessionId,
    pub cwd: PathBuf,
    /// Read *before* replay: it decides whether reasoning blocks may be echoed,
    /// so resuming under a different model has to be a deliberate act.
    pub model: String,
    pub started: String,
    /// One line from the first prompt. Without it `--list` is a wall of ULIDs.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub axio: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "rec", rename_all = "snake_case")]
pub enum Record {
    Header {
        #[serde(flatten)]
        header: Header,
    },
    /// Flattened, so a line stays greppable: `{"rec":"item","id":…,"item":"tool_call",…}`.
    Item {
        #[serde(flatten)]
        item: Item,
    },
    /// Observability only. Never folded into the transcript — compaction is
    /// re-derived on load, not replayed.
    Compacted {
        stage: u8,
        dropped: u32,
        tokens_before: u64,
        tokens_after: u64,
    },
    /// What a turn cost. Written once per turn, because otherwise "what did
    /// this session cost" is unanswerable for every session already on disk.
    TurnEnded {
        outcome: TurnOutcome,
        usage: Usage,
        cost_usd: f64,
    },
    /// Written at the head of a resumed run, so an append-only file is
    /// self-describing without ever rewriting line one.
    Resumed { at_ms: u64, model: String },
    /// A record kind a newer axio writes. Skipped, never fatal.
    #[serde(other)]
    Unknown,
}

/// Is this item part of the durable history?
///
/// The rule is "what `wire_messages` would replay", minus two:
///
/// * an empty `AgentMessage`, which the wire rejects anyway;
/// * a `ContextElision`, which describes the in-memory projection rather than
///   the history. The file still contains every item such a marker claims was
///   removed, so persisting one writes a lie — and because the file is
///   append-only, every resume would append another.
pub fn is_persisted(item: &Item) -> bool {
    match &item.body {
        ItemBody::AgentMessage { text } => !text.is_empty(),
        ItemBody::ContextElision { .. } => false,
        _ => true,
    }
}

/// Where session records go, or don't.
///
/// An enum rather than an `Option<SessionFile>`: there is no arm a call site
/// can forget to handle.
#[derive(Debug)]
pub enum Recorder {
    /// `--ephemeral`. Every append is a no-op and no directory is created.
    Ephemeral,
    File(SessionFile),
}

impl Recorder {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Recorder::Ephemeral => None,
            Recorder::File(f) => Some(&f.path),
        }
    }

    pub fn append(&mut self, record: &Record) {
        if let Recorder::File(file) = self {
            file.append(record);
        }
    }

    /// Record an item if it is durable. The `is_persisted` check lives here so
    /// no caller has to remember it.
    pub fn append_item(&mut self, item: &Item) {
        if is_persisted(item) {
            self.append(&Record::Item { item: item.clone() });
        }
    }
}

#[derive(Debug)]
pub struct SessionFile {
    file: std::fs::File,
    path: PathBuf,
    /// Set by a failed or short write. The next append is prefixed with a
    /// newline so a torn line becomes one skippable bad line rather than
    /// corrupting the record that follows it.
    dirty: bool,
}

impl SessionFile {
    /// Create the file and write its header in the same call, so a header can
    /// never appear after an item and a concurrent reader sees either a
    /// complete header or a zero-byte file.
    pub fn create(path: PathBuf, header: &Header) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        let mut this = Self {
            file,
            path,
            dirty: false,
        };
        this.append(&Record::Header {
            header: header.clone(),
        });
        Ok(this)
    }

    /// Open an existing file to continue appending to it.
    pub fn reopen(path: PathBuf) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new().append(true).open(&path)?;
        Ok(Self {
            file,
            path,
            dirty: false,
        })
    }

    /// Append one record, flushing immediately.
    ///
    /// Unbuffered and flushed per record, not `sync_all`: the cost of an fsync
    /// per line is not worth paying, and after a crash the file is whole lines
    /// either way. A write failure is recorded rather than propagated — losing
    /// the log is not a reason to fail the turn the user is waiting on.
    pub fn append(&mut self, record: &Record) {
        let Ok(mut line) = serde_json::to_string(record) else {
            // A record that will not serialise is a bug in axio, not a broken
            // file: do not mark the file dirty for it.
            tracing::error!("a session record could not be serialised");
            return;
        };
        if self.dirty {
            line.insert(0, '\n');
            self.dirty = false;
        }
        line.push('\n');
        if let Err(e) = self.file.write_all(line.as_bytes()) {
            tracing::warn!("session write failed: {e}");
            self.dirty = true;
            return;
        }
        if let Err(e) = self.file.flush() {
            tracing::warn!("session flush failed: {e}");
            self.dirty = true;
        }
    }
}

/// A session read back from disk.
#[derive(Debug)]
pub struct Loaded {
    pub session: Session,
    pub header: Header,
    pub notices: Vec<Notice>,
    /// A record was lost from the middle of the file, not just the tail. The
    /// transcript has a hole the model would misread as history.
    pub degraded: bool,
}

/// Read a session file back into a transcript.
///
/// Never fails on a damaged line. A truncated final line is a crash, not
/// corruption — erroring there would lose the user's entire session over the
/// last few bytes.
pub fn load(path: &Path) -> std::io::Result<Loaded> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);

    let mut notices = Vec::new();
    let mut header: Option<Header> = None;
    let mut items: Vec<Item> = Vec::new();
    let mut degraded = false;

    let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
    let last = lines.len().saturating_sub(1);

    for (n, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Record = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(e) => {
                if n == last {
                    notices.push(Notice::warn(
                        "the last line of this session was incomplete and was skipped; \
                         it was most likely interrupted mid-write",
                    ));
                } else {
                    notices.push(Notice::error(format!(
                        "line {} of this session could not be read ({e}); it was skipped",
                        n + 1
                    )));
                    degraded = true;
                }
                continue;
            }
        };

        match record {
            Record::Header { header: h } => {
                if h.version > SESSION_FORMAT_VERSION {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "this session was written by a newer axio (format {} > {}); \
                             upgrade to read it",
                            h.version, SESSION_FORMAT_VERSION
                        ),
                    ));
                }
                header = Some(h);
            }
            // Replacing by id is the fold: a tool call is written once when the
            // model emits it and again as its status resolves.
            Record::Item { item } => match items.iter_mut().find(|i| i.id == item.id) {
                Some(existing) => *existing = item,
                None => items.push(item),
            },
            Record::Compacted { .. } | Record::TurnEnded { .. } | Record::Resumed { .. } => {}
            Record::Unknown => {}
        }
    }

    let Some(header) = header else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "this session file has no header line",
        ));
    };

    let mut session =
        Session::from_parts(header.id, header.cwd.clone(), header.model.clone(), items);

    // A `tool_use` with no matching `tool_result` is rejected outright, so a
    // session interrupted mid-call cannot be resumed until every call has an
    // answer. The wording matters: the call may well have completed.
    let orphans = session.unfinished_calls();
    if !orphans.is_empty() {
        for call_id in &orphans {
            session.set_tool_status(call_id, ToolStatus::Cancelled);
        }
        notices.push(Notice::warn(format!(
            "{} tool call(s) had no recorded result and were marked cancelled; \
             they may have completed before the interruption",
            orphans.len()
        )));
    }

    Ok(Loaded {
        session,
        header,
        notices,
        degraded,
    })
}

/// Read only the header of a session file.
///
/// One line, one deserialisation, a bounded read. This is what makes `--list`
/// cheap enough to stay useful as sessions accumulate.
pub fn read_header(path: &Path) -> std::io::Result<Header> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file.take(HEADER_MAX_BYTES));
    let mut line = String::new();
    reader.read_line(&mut line)?;

    match serde_json::from_str::<Record>(line.trim()) {
        Ok(Record::Header { header }) => Ok(header),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no header on the first line",
        )),
    }
}

use std::io::Read;

/// Where sessions live on disk.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file for a session.
    ///
    /// The directory comes from the id's own timestamp, so resuming never has
    /// to scan, and the file name is the re-encoded id — a ULID cannot contain
    /// a path separator or `..`, so a hostile `--resume` argument dies at
    /// parsing rather than at `join`.
    pub fn path_for(&self, id: SessionId) -> PathBuf {
        self.root.join(day_of(id)).join(format!("{id}.jsonl"))
    }

    /// Every session file, newest first.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(days) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for day in days.filter_map(Result::ok) {
            let Ok(entries) = std::fs::read_dir(day.path()) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "jsonl") {
                    out.push(path);
                }
            }
        }
        // The file name is the ULID, which sorts lexicographically by time.
        out.sort();
        out.reverse();
        out
    }

    /// Resolve a full or partial id.
    ///
    /// A 26-character identifier typed by hand is a usability failure, and
    /// resolving a prefix needs only the file names — no header is read.
    pub fn resolve(&self, needle: &str) -> Result<SessionId, String> {
        let needle = needle.trim().to_ascii_uppercase();
        if needle.is_empty() {
            return Err("no session id given".into());
        }
        if let Ok(id) = needle.parse::<SessionId>() {
            return Ok(id);
        }

        let matches: Vec<SessionId> = self
            .files()
            .iter()
            .filter_map(|p| p.file_stem()?.to_str()?.parse::<SessionId>().ok())
            .filter(|id| id.to_string().starts_with(&needle))
            .collect();

        match matches.len() {
            0 => Err(format!("no session matches `{needle}`")),
            1 => Ok(matches[0]),
            n => Err(format!(
                "`{needle}` matches {n} sessions; give more characters"
            )),
        }
    }
}

/// The `yyyy-mm-dd` a session id was minted on, from the id itself.
fn day_of(id: SessionId) -> String {
    let ms = id.timestamp_ms();
    let days = (ms / 86_400_000) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since the epoch to a calendar date.
///
/// Hand-rolled rather than pulling in a date library for one call: the core
/// crate's dependency count is a property worth keeping.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::NoticeLevel;

    fn header(id: SessionId) -> Header {
        Header {
            version: SESSION_FORMAT_VERSION,
            protocol: crate::protocol::PROTOCOL_VERSION,
            id,
            cwd: PathBuf::from("/w"),
            model: "claude-opus-5".into(),
            started: "2026-07-26T00:00:00Z".into(),
            label: Some("do a thing".into()),
            axio: "0.1.0-dev".into(),
        }
    }

    fn tool_item(call_id: &str, status: ToolStatus) -> Item {
        Item::new(ItemBody::ToolCall {
            call_id: call_id.into(),
            name: "read".into(),
            input: serde_json::json!({"path": "a.rs"}),
            subject: "read:a.rs".into(),
            preview: None,
            status,
        })
    }

    #[test]
    fn a_header_is_written_before_any_item() {
        let dir = tempfile::tempdir().unwrap();
        let id = SessionId::generate();
        let path = dir.path().join("s.jsonl");
        let mut f = SessionFile::create(path.clone(), &header(id)).unwrap();
        f.append(&Record::Item {
            item: Item::new(ItemBody::UserMessage { text: "hi".into() }),
        });

        let text = std::fs::read_to_string(&path).unwrap();
        let first = text.lines().next().unwrap();
        assert!(first.contains("\"rec\":\"header\""), "{first}");
        assert_eq!(read_header(&path).unwrap().id, id);
    }

    #[test]
    fn an_item_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let id = SessionId::generate();
        let path = dir.path().join("s.jsonl");
        let mut f = SessionFile::create(path.clone(), &header(id)).unwrap();
        f.append(&Record::Item {
            item: Item::new(ItemBody::UserMessage {
                text: "hello".into(),
            }),
        });
        f.append(&Record::Item {
            item: tool_item(
                "toolu_1",
                ToolStatus::Ok {
                    output: "contents".into(),
                    truncated: false,
                    spill: None,
                    ms: 4,
                },
            ),
        });

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.session.transcript().len(), 2);
        assert!(loaded.notices.is_empty());
        assert!(!loaded.degraded);
    }

    #[test]
    fn a_tool_call_is_replaced_by_id_not_appended_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut f = SessionFile::create(path.clone(), &header(SessionId::generate())).unwrap();

        let mut item = tool_item("toolu_1", ToolStatus::Pending);
        f.append(&Record::Item { item: item.clone() });
        if let ItemBody::ToolCall { status, .. } = &mut item.body {
            *status = ToolStatus::Ok {
                output: "done".into(),
                truncated: false,
                spill: None,
                ms: 1,
            };
        }
        f.append(&Record::Item { item: item.clone() });

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.session.transcript().len(), 1, "one call, one item");
        assert!(matches!(
            loaded.session.transcript()[0].body,
            ItemBody::ToolCall {
                status: ToolStatus::Ok { .. },
                ..
            }
        ));
    }

    #[test]
    fn a_truncated_final_line_is_a_notice_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        {
            let mut f = SessionFile::create(path.clone(), &header(SessionId::generate())).unwrap();
            f.append(&Record::Item {
                item: Item::new(ItemBody::UserMessage {
                    text: "kept".into(),
                }),
            });
        }
        // Simulate a kill mid-write.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"rec\":\"item\",\"id\":\"01K\",\"item\":\"user_mes");
        std::fs::write(&path, text).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(
            loaded.session.transcript().len(),
            1,
            "the good item survives"
        );
        assert_eq!(loaded.notices.len(), 1);
        assert_eq!(loaded.notices[0].level, NoticeLevel::Warn);
        assert!(!loaded.degraded, "a torn tail is a crash, not a hole");
    }

    #[test]
    fn a_bad_line_in_the_middle_marks_the_session_degraded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        {
            let mut f = SessionFile::create(path.clone(), &header(SessionId::generate())).unwrap();
            f.append(&Record::Item {
                item: Item::new(ItemBody::UserMessage { text: "one".into() }),
            });
        }
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{ not json at all\n");
        text.push_str(
            &serde_json::to_string(&Record::Item {
                item: Item::new(ItemBody::UserMessage { text: "two".into() }),
            })
            .unwrap(),
        );
        text.push('\n');
        std::fs::write(&path, text).unwrap();

        let loaded = load(&path).unwrap();
        assert!(loaded.degraded, "a hole in the middle is not a torn tail");
        assert_eq!(loaded.session.transcript().len(), 2);
    }

    #[test]
    fn an_orphan_tool_call_is_repaired_on_load() {
        // A tool_use with no result is rejected outright, so an interrupted
        // session cannot be resumed until every call has an answer.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        {
            let mut f = SessionFile::create(path.clone(), &header(SessionId::generate())).unwrap();
            f.append(&Record::Item {
                item: tool_item("toolu_1", ToolStatus::Running),
            });
        }

        let loaded = load(&path).unwrap();
        assert!(loaded.session.unfinished_calls().is_empty());
        assert!(
            loaded
                .notices
                .iter()
                .any(|n| n.message.contains("cancelled"))
        );
        // And the repaired transcript produces a valid request.
        let wire = loaded.session.wire_messages("claude-opus-5");
        assert!(wire.iter().any(|m| {
            m.content
                .iter()
                .any(|c| matches!(c, crate::provider::WireContent::ToolResult { .. }))
        }));
    }

    #[test]
    fn a_newer_format_is_refused_rather_than_misread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut h = header(SessionId::generate());
        h.version = SESSION_FORMAT_VERSION + 1;
        SessionFile::create(path.clone(), &h).unwrap();

        let err = load(&path).unwrap_err();
        assert!(err.to_string().contains("newer axio"), "{err}");
    }

    #[test]
    fn an_unknown_record_kind_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        {
            SessionFile::create(path.clone(), &header(SessionId::generate())).unwrap();
        }
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"rec\":\"invented_by_a_later_version\",\"x\":1}\n");
        std::fs::write(&path, text).unwrap();

        let loaded = load(&path).unwrap();
        assert!(
            loaded.notices.is_empty(),
            "forward compatibility, not a warning"
        );
        assert!(!loaded.degraded);
    }

    #[test]
    fn an_elision_marker_is_never_persisted() {
        // The file still holds every item such a marker claims was removed, so
        // writing one is a lie — and append-only means every resume adds
        // another.
        assert!(!is_persisted(&Item::new(ItemBody::ContextElision {
            dropped_items: 12
        })));
        assert!(!is_persisted(&Item::new(ItemBody::AgentMessage {
            text: String::new()
        })));
        assert!(is_persisted(&Item::new(ItemBody::AgentMessage {
            text: "real".into()
        })));
    }

    #[test]
    fn ephemeral_writes_nothing_at_all() {
        let mut r = Recorder::Ephemeral;
        r.append_item(&Item::new(ItemBody::UserMessage { text: "hi".into() }));
        assert!(r.path().is_none());
    }

    #[test]
    fn a_session_path_is_derived_from_the_id_alone() {
        let store = SessionStore::new(PathBuf::from("/state/sessions"));
        let id: SessionId = "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap();
        let path = store.path_for(id);
        assert!(
            path.to_string_lossy()
                .ends_with("01ARZ3NDEKTSV4RRFFQ69G5FAV.jsonl")
        );
        // The day directory comes from the id's own timestamp.
        assert!(
            path.parent()
                .unwrap()
                .to_string_lossy()
                .contains("2016-07-30")
        );
    }

    #[test]
    fn a_hostile_resume_argument_cannot_escape_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        for hostile in ["../../etc/passwd", "..", "/etc/passwd", "a/b"] {
            assert!(
                store.resolve(hostile).is_err(),
                "`{hostile}` must not resolve to a session"
            );
        }
    }

    #[test]
    fn a_prefix_resolves_when_it_is_unambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let id = SessionId::generate();
        SessionFile::create(store.path_for(id), &header(id)).unwrap();

        let text = id.to_string();
        assert_eq!(store.resolve(&text[..8]).unwrap(), id);
        assert_eq!(store.resolve(&text).unwrap(), id);
        assert!(store.resolve("ZZZZZZZZ").is_err());
    }

    #[test]
    fn listing_reads_one_line_per_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());

        for _ in 0..20 {
            let id = SessionId::generate();
            let mut f = SessionFile::create(store.path_for(id), &header(id)).unwrap();
            // A transcript far longer than the header.
            for n in 0..200 {
                f.append(&Record::Item {
                    item: Item::new(ItemBody::UserMessage {
                        text: format!("line {n} with enough text to matter"),
                    }),
                });
            }
        }

        let files = store.files();
        assert_eq!(files.len(), 20);
        let headers: Vec<Header> = files.iter().filter_map(|p| read_header(p).ok()).collect();
        assert_eq!(headers.len(), 20);
        // Newest first, so `--list` shows recent work without a sort.
        let ids: Vec<String> = files
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.reverse();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn a_date_is_derived_correctly_from_a_ulid() {
        // A known ULID timestamp, so a broken calendar conversion cannot hide.
        let id: SessionId = "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap();
        assert_eq!(day_of(id), "2016-07-30");
    }
}
