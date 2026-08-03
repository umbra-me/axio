//! One module per agent, and a registry that walks them all.
//!
//! Every agent writes its own transcript format, so each parser is bespoke. What they
//! share is the output — [`CostMessage`] — and the discovery shape declared by [`Source`]:
//! where the logs live, and how to turn one file into messages.
//!
//! # Reading someone else's log is a best-effort operation
//!
//! These files belong to other programs. They are appended to while we read, truncated on
//! crash, and reformatted without notice on the vendor's schedule. A parser that stops at
//! the first surprise reports nothing for a whole agent because of one bad line, so the
//! rule throughout is: **a malformed line costs that line, not the file, and a malformed
//! file costs that file, not the scan.** Everything skipped is counted in [`ScanReport`]
//! so `--diagnose` can say what was dropped rather than quietly reporting a smaller number.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::message::{CostMessage, DedupLedger};

pub mod catalog;
pub mod claude_code;
#[cfg(feature = "sqlite")]
pub mod database;
pub mod codex;
pub mod generic;
pub mod grok;
pub mod usage;

/// An agent whose sessions can be read from disk.
pub trait Source: Send + Sync {
    /// Stable identifier used in output and in `--by client` grouping.
    fn client(&self) -> &'static str;

    /// Human-facing name for tables and the Cost view.
    fn display_name(&self) -> &'static str;

    /// Directories to walk, given the user's home. Returned whether or not they exist —
    /// the caller reports a missing directory as *this agent is not installed*, which is
    /// a different answer from *this agent recorded nothing*.
    fn roots(&self, home: &Path) -> Vec<PathBuf>;

    /// Whether this file is one this parser understands.
    fn owns(&self, path: &Path) -> bool {
        path.extension().is_some_and(|ext| ext == "jsonl")
    }

    /// Parse one file, pushing messages into `out`.
    ///
    /// The ledger is shared across the whole agent rather than per file because some
    /// agents split one session over several files, and a response repeated across a
    /// split would otherwise be billed twice.
    fn parse(&self, path: &Path, contents: &str, out: &mut DedupLedger) -> FileOutcome;

    /// Open and parse a file, for sources whose store is not text.
    ///
    /// The default reads the file and hands the text to [`Source::parse`], which is what
    /// every JSONL agent wants. A source backed by a database overrides this instead and
    /// leaves `parse` unimplemented — a SQLite file has no meaningful `&str` form, and
    /// forcing one through the text path would mean loading a database into a String and
    /// then refusing to use it.
    fn open(&self, path: &Path, out: &mut DedupLedger) -> Option<FileOutcome> {
        let contents = std::fs::read_to_string(path).ok()?;
        Some(self.parse(path, &contents, out))
    }
}

/// What reading one file produced.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOutcome {
    /// Lines that parsed and carried billable usage.
    pub billable: usize,
    /// Lines that were understood but carried nothing to bill — user turns, tool results,
    /// metadata. Not a problem; counted so the totals add up.
    pub skipped: usize,
    /// Lines that could not be parsed at all.
    pub malformed: usize,
}

impl FileOutcome {
    fn merge(&mut self, other: FileOutcome) {
        self.billable += other.billable;
        self.skipped += other.skipped;
        self.malformed += other.malformed;
    }
}

/// What a whole scan produced, per agent.
#[derive(Debug, Clone)]
pub struct AgentReport {
    pub client: &'static str,
    pub display_name: &'static str,
    /// False when none of the agent's directories exist — it is not installed.
    pub present: bool,
    pub files_read: usize,
    /// Files that existed but could not be read at all (permissions, a lock, a device).
    pub files_failed: usize,
    pub outcome: FileOutcome,
    pub messages: Vec<CostMessage>,
}

/// The result of walking every registered agent.
#[derive(Debug, Default, Clone)]
pub struct ScanReport {
    pub agents: Vec<AgentReport>,
}

impl ScanReport {
    /// Every billable message found, across all agents.
    pub fn messages(&self) -> impl Iterator<Item = &CostMessage> {
        self.agents.iter().flat_map(|agent| agent.messages.iter())
    }

    pub fn installed(&self) -> impl Iterator<Item = &AgentReport> {
        self.agents.iter().filter(|agent| agent.present)
    }
}

/// Every agent this build can read.
///
/// Ordered so the output is stable between runs; a cost table that reorders itself is one
/// nobody can diff against yesterday's.
pub fn registry() -> Vec<Box<dyn Source>> {
    let mut sources: Vec<Box<dyn Source>> = vec![
        Box::new(claude_code::ClaudeCode),
        Box::new(codex::Codex),
        Box::new(grok::Grok),
    ];
    // The hand-written three come first because they carry format knowledge the
    // table-driven walk cannot: which of two token figures is cumulative, which repeats
    // must be suppressed, which vendor reports its own cost. The catalog covers the rest.
    sources.extend(
        catalog::CATALOG
            .iter()
            .map(|agent| Box::new(*agent) as Box<dyn Source>),
    );
    #[cfg(feature = "sqlite")]
    sources.extend(
        database::CATALOG
            .iter()
            .map(|agent| Box::new(*agent) as Box<dyn Source>),
    );
    sources
}

/// How many workers to split a scan across.
///
/// The work is reading and parsing thousands of small files, so it is bound by neither
/// CPU nor disk alone and oversubscribing slightly beats matching core count exactly.
/// Capped because the gain flattens well before the thread count stops rising, and an
/// agent with four files should not spawn thirty-two threads to read them.
/// `AXIO_COST_THREADS` overrides it, which is how the parallel and sequential paths get
/// compared on the same machine — and how a scan gets pinned to one thread when a parser
/// is being debugged and interleaved output would be unreadable.
fn worker_count(files: usize) -> usize {
    if let Some(forced) = std::env::var("AXIO_COST_THREADS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|count| *count > 0)
    {
        return forced.min(files.max(1));
    }
    let cores = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    cores.clamp(1, 16).min(files.max(1))
}

/// What one worker produced for one agent.
type Part = (DedupLedger, FileOutcome, usize, usize);

/// Walk every agent's log directories under `home`.
///
/// Two phases over one flat pool of workers, and the flatness is the point. The obvious
/// shape — a thread per agent, each splitting its own files again — spawns the *product*
/// of the two counts: twenty-two agents against sixteen workers each reserves stacks for
/// over a hundred threads on a machine that can run eight. It also balances badly, because
/// the split is per agent: one agent here holds 449 files and another holds 2, and the
/// second gets a whole worker while the first queues.
///
/// So the file lists are gathered first, concatenated, and split once. Every worker gets
/// the same number of files whoever wrote them, and the thread count is `worker_count`.
pub fn scan(home: &Path, sources: &[Box<dyn Source>]) -> ScanReport {
    // Phase 1: find the files. Walking a directory is IO-bound and short, and it has to
    // finish before the parse can be split evenly — you cannot balance a list you have
    // not counted.
    let found = list_files(home, sources);

    let jobs: Vec<(usize, &Path)> = found
        .iter()
        .enumerate()
        .flat_map(|(index, (_, paths))| paths.iter().map(move |path| (index, path.as_path())))
        .collect();

    // Phase 2: parse. Chunked rather than work-stolen: these files are of similar size, so
    // a static split costs nothing and needs no shared queue.
    let chunk = jobs.len().div_ceil(worker_count(jobs.len()).max(1)).max(1);
    let parts: Vec<HashMap<usize, Part>> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|batch| scope.spawn(|| parse_batch(batch, sources)))
            .collect();
        // A worker that panicked yields nothing rather than bringing down the scan: one
        // agent's malformed store must not cost the other twenty-one their numbers.
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    });

    let mut agents: Vec<AgentReport> = found
        .iter()
        .zip(sources)
        .map(|((present, _), source)| AgentReport {
            client: source.client(),
            display_name: source.display_name(),
            present: *present,
            files_read: 0,
            files_failed: 0,
            outcome: FileOutcome::default(),
            messages: Vec::new(),
        })
        .collect();
    let mut ledgers: Vec<DedupLedger> = (0..sources.len()).map(|_| DedupLedger::new()).collect();

    // Merged in chunk order, and each chunk's agents in index order, so an agent's
    // messages stay in the order its files were walked however the work was divided. A
    // report that reorders itself between runs is one nobody can diff against yesterday's.
    for part in parts {
        let mut owned: Vec<(usize, Part)> = part.into_iter().collect();
        owned.sort_by_key(|(index, _)| *index);
        for (index, (ledger, outcome, read, failed)) in owned {
            ledgers[index].absorb(ledger);
            agents[index].outcome.merge(outcome);
            agents[index].files_read += read;
            agents[index].files_failed += failed;
        }
    }
    for (agent, ledger) in agents.iter_mut().zip(ledgers) {
        agent.messages = ledger.into_messages();
    }

    ScanReport { agents }
}

/// Every agent's files, and whether the agent is installed at all.
///
/// Two separate answers: an agent that is installed and has recorded nothing is not the
/// same fact as one that is absent, and neither produces a file.
fn list_files(home: &Path, sources: &[Box<dyn Source>]) -> Vec<(bool, Vec<PathBuf>)> {
    let chunk = sources
        .len()
        .div_ceil(worker_count(sources.len()).max(1))
        .max(1);

    std::thread::scope(|scope| {
        let handles: Vec<_> = sources
            .chunks(chunk)
            .map(|batch| {
                scope.spawn(move || {
                    batch
                        .iter()
                        .map(|source| {
                            let roots = source.roots(home);
                            let present = roots.iter().any(|root| root.exists());
                            let paths = roots
                                .iter()
                                .filter(|root| root.exists())
                                .flat_map(|root| walk(root))
                                .filter(|path| source.owns(path))
                                .collect();
                            (present, paths)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

/// Read one worker's share, keeping a separate ledger per agent.
///
/// Per agent rather than per batch because dedup is a property of one agent's log format:
/// two agents can legitimately emit the same key for unrelated work, and a shared ledger
/// would silently drop the second.
fn parse_batch(batch: &[(usize, &Path)], sources: &[Box<dyn Source>]) -> HashMap<usize, Part> {
    let mut parts: HashMap<usize, Part> = HashMap::new();
    for (index, path) in batch {
        let entry = parts
            .entry(*index)
            .or_insert_with(|| (DedupLedger::new(), FileOutcome::default(), 0, 0));
        match sources[*index].open(path, &mut entry.0) {
            Some(file) => {
                entry.2 += 1;
                entry.1.merge(file);
            }
            // A file we cannot read is worth counting but never worth stopping for: an
            // agent running right now may hold a lock on its newest transcript, which is
            // the common case, not an error.
            None => entry.3 += 1,
        }
    }
    parts
}

/// Every file beneath `root`, following the walker's defaults.
///
/// Hidden files are included — `~/.codex` and `~/.claude` are themselves hidden, so a
/// walker that skips dotted entries would find nothing at all. Ignore files are not
/// consulted: these are data directories, and a stray `.gitignore` in one should not
/// silently subtract from someone's bill.
fn walk(root: &Path) -> impl Iterator<Item = PathBuf> {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|file| file.is_file()))
        .map(|entry| entry.into_path())
}

/// Read a `YYYY-MM-DD` date out of an RFC 3339 timestamp without parsing it.
///
/// Every one of these logs writes RFC 3339, where the date is the first ten characters by
/// definition. Pricing needs the day and nothing finer, and this runs once per message.
pub fn date_of(timestamp: &str) -> &str {
    if timestamp.len() >= 10 { &timestamp[..10] } else { timestamp }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_has_no_duplicate_client_ids() {
        let registry = registry();
        let mut ids: Vec<_> = registry.iter().map(|source| source.client()).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "client ids are the grouping key");
    }

    #[test]
    fn an_absent_agent_is_reported_as_not_installed() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let report = scan(dir.path(), &registry());
        assert!(report.agents.iter().all(|agent| !agent.present));
        assert_eq!(report.messages().count(), 0);
    }

    #[test]
    fn dates_come_off_the_front_of_a_timestamp() {
        assert_eq!(date_of("2026-08-02T10:30:00.134643Z"), "2026-08-02");
        assert_eq!(date_of("short"), "short");
    }
}
