//! The Cost view's data, and the cache that makes it usable.
//!
//! Scanning every agent's transcripts takes tens of seconds on a machine with real
//! history — 1,771 files and 18 billion tokens on the one this was written on. A window
//! that re-scanned on every tab click would be unusable, so the scan happens once and the
//! grouping, which is cheap, happens per call.
//!
//! The scan runs on a worker and is saved to disk when it finishes, so a relaunch shows
//! last session's figures immediately and replaces them a moment later. Sessions are
//! append-only and minutes old at worst, and a cost figure that is a few minutes stale is
//! not wrong in any way that matters — whereas a window that blocks for half a minute is.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axio_cost::pricing::Prices;
use axio_cost::sources::{registry, scan};
use axio_cost::totals::{Cost, Totals};
use axio_cost::{CostMessage, ScanReport};
use serde::Serialize;

/// Emitted whenever the scan behind the Cost and Stats views changes — once when the
/// saved scan is published, once when a live scan replaces it. The frontend listens
/// rather than polling, because the two arrive seconds apart and a poll interval short
/// enough to catch that would be running constantly for the rest of the session.
pub const EVENT_COST_UPDATED: &str = "cost://updated";

/// One row of the table, already rendered into what the view needs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostRow {
    pub key: String,
    pub messages: usize,
    pub tokens: u64,
    /// `None` when too little of the row could be priced to mean anything. The view shows
    /// "unpriced" rather than a number, for the reason `axio_cost::totals` documents.
    pub cost_usd: Option<f64>,
    /// Share of this row's tokens that carry a price, `0.0..=1.0`.
    pub coverage: f64,
}

/// One day of the calendar, flattened for the view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayPoint {
    /// `YYYY-MM-DD`, UTC. The view builds the grid from this rather than being handed
    /// empty days, so the two cannot disagree about what a week is.
    pub date: String,
    pub messages: usize,
    pub tokens: u64,
    pub cost_usd: Option<f64>,
}

/// The year, and what its shape says.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsView {
    pub days: Vec<DayPoint>,
    pub busiest_day_tokens: u64,
    /// The three cuts that split a day into one of four heat levels. Computed here so the
    /// window and the CLI shade the same day the same way — see `Stats::thresholds`.
    pub thresholds: [u64; 3],
    pub active_days: usize,
    pub current_streak: usize,
    pub longest_streak: usize,
    pub sessions: usize,
    pub top_model: Option<String>,
    pub total_tokens: u64,
    pub total_cost_usd: Option<f64>,
    /// Tokens by hour of day, UTC — labelled as UTC in the view for the reason
    /// `Stats::by_hour` documents.
    pub by_hour: Vec<u64>,
    /// Tokens by weekday, Sunday first, so it lines up with the calendar above it.
    pub by_weekday: Vec<u64>,
    /// Where the money went, three ways. Each is the same grouping the Cost tab offers,
    /// cut to the rows worth drawing as bars — a chart with forty bars is a table.
    pub by_provider: Vec<CostRow>,
    pub by_harness: Vec<CostRow>,
    pub by_workspace: Vec<CostRow>,
    /// What the tokens actually were. Cache reads are the majority of a coding agent's
    /// volume and a tenth of its price, so a total that does not separate them invites
    /// the wrong conclusion about where the money goes.
    pub mix: TokenMix,
}

/// The four kinds of token, which are billed at four different rates.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenMix {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Output tokens spent on reasoning. A subset of `output`, never billed separately —
    /// shown because it is the part of a bill nobody remembers asking for.
    pub reasoning: u64,
}

/// The saved scan, as the Settings tab describes it.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredScan {
    pub path: String,
    /// Zero when nothing has been written yet, which the view reads as "not saved".
    pub bytes: u64,
    pub scanned_at: Option<String>,
    pub scanning: bool,
}

/// What one agent contributed, for the view's footer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    pub name: &'static str,
    /// False when the agent is not installed — a different answer from "recorded nothing".
    pub present: bool,
    pub files: usize,
    pub messages: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostReport {
    pub rows: Vec<CostRow>,
    pub total: CostRow,
    pub unpriced_models: Vec<String>,
    pub agents: Vec<AgentRow>,
    /// True while nothing has been scanned yet, so the view can say so rather than
    /// showing an empty table that looks like a zero.
    pub loading: bool,
    /// True while a scan is running behind this answer. Distinct from `loading`: these
    /// figures are real, they are just about to be replaced by fresher ones.
    pub scanning: bool,
}

impl CostReport {
    fn empty() -> Self {
        CostReport {
            rows: Vec::new(),
            total: CostRow {
                key: "total".into(),
                messages: 0,
                tokens: 0,
                cost_usd: None,
                coverage: 1.0,
            },
            unpriced_models: Vec::new(),
            agents: Vec::new(),
            loading: true,
            scanning: true,
        }
    }
}

/// The scan, held so the window can regroup without paying for it again.
///
/// Nothing here scans on the calling thread, and that is the entire point of the type.
/// A Tauri command declared `fn` rather than `async fn` runs on the main thread, which is
/// also the thread that paints and handles input — so the previous version, which scanned
/// inline on first use, froze the window solid for the half-minute a scan takes. Worse, it
/// held this mutex for the duration, so every other Cost call queued behind it.
///
/// Scanning happens on a worker started by [`CostCache::rescan`]. The lock is taken twice,
/// briefly: once to publish a warm cache from disk, once to swap in the finished scan.
pub struct CostCache {
    scanned: Mutex<Option<ScanReport>>,
    /// True from the moment a worker starts until it swaps its result in. Also the
    /// interlock that keeps two rescans from walking the same files at once.
    scanning: AtomicBool,
    /// Where a finished scan is written so the next launch has something to show
    /// immediately. Machine-local: this is derived data, and large.
    cache_file: PathBuf,
}

impl CostCache {
    pub fn new(cache_file: PathBuf) -> Self {
        CostCache {
            scanned: Mutex::new(None),
            scanning: AtomicBool::new(false),
            cache_file,
        }
    }

    /// Start a scan on a worker thread, unless one is already running.
    ///
    /// Two phases, because they answer different questions. The saved scan is published
    /// first so the window has real figures within milliseconds of launch; the live scan
    /// then replaces it. Anyone watching sees last session's numbers, then this session's
    /// — never an empty table, and never a frozen window.
    ///
    /// `finished` is called after each phase that produced data, so the caller can tell
    /// the frontend to reload.
    pub fn rescan(self: &Arc<Self>, finished: impl Fn() + Send + 'static) {
        if self.scanning.swap(true, Ordering::SeqCst) {
            return;
        }
        let cache = Arc::clone(self);
        std::thread::spawn(move || {
            if cache.warm() {
                finished();
            }
            if let Some(home) = home_dir() {
                // `scan` is already parallel across agents — see `axio_cost::sources`.
                // What it was missing was a thread of its own to be parallel *on*.
                let fresh = scan(&home, &registry());
                let _ = axio_cost::store::save(&cache.cache_file, &fresh);
                *cache.scanned.lock().unwrap_or_else(|err| err.into_inner()) = Some(fresh);
            }
            cache.scanning.store(false, Ordering::SeqCst);
            finished();
        });
    }

    /// Publish the saved scan if there is one and nothing better is loaded yet.
    fn warm(&self) -> bool {
        let mut cached = self.scanned.lock().unwrap_or_else(|err| err.into_inner());
        if cached.is_some() {
            return false;
        }
        match axio_cost::store::load(&self.cache_file) {
            Some(saved) => {
                *cached = Some(saved.report);
                true
            }
            None => false,
        }
    }

    fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::SeqCst)
    }

    /// Group whatever has been scanned so far.
    pub fn report(&self, group: &str) -> CostReport {
        let cached = self.scanned.lock().unwrap_or_else(|err| err.into_inner());
        let Some(report) = cached.as_ref() else {
            return CostReport::empty();
        };
        let mut report = group_report(report, group);
        report.scanning = self.is_scanning();
        report
    }

    /// The calendar and the habit it implies, from the same cached scan.
    pub fn stats(&self) -> StatsView {
        let cached = self.scanned.lock().unwrap_or_else(|err| err.into_inner());
        let Some(report) = cached.as_ref() else {
            return StatsView::default();
        };

        let stats = axio_cost::stats::summarise(report.messages(), &Prices::bundled());
        StatsView {
            busiest_day_tokens: stats.busiest(),
            thresholds: stats.thresholds(),
            days: stats
                .days
                .iter()
                .map(|day| DayPoint {
                    date: day.date.to_string(),
                    messages: day.messages,
                    tokens: day.tokens,
                    cost_usd: day.cost,
                })
                .collect(),
            active_days: stats.active_days,
            current_streak: stats.current_streak,
            longest_streak: stats.longest_streak,
            sessions: stats.sessions,
            top_model: stats.top_model.clone(),
            total_tokens: stats.totals.tokens.total(),
            total_cost_usd: stats.totals.cost().partial().map(|(dollars, _)| dollars),
            by_hour: stats.by_hour.to_vec(),
            by_weekday: stats.by_weekday.to_vec(),
            by_provider: top_rows(report, "provider", 6),
            by_harness: top_rows(report, "harness", 6),
            by_workspace: top_rows(report, "workspace", 8),
            mix: TokenMix {
                input: stats.totals.tokens.input,
                output: stats.totals.tokens.output,
                cache_read: stats.totals.tokens.cache_read,
                cache_write: stats.totals.tokens.cache_write_5m
                    + stats.totals.tokens.cache_write_1h,
                reasoning: stats.totals.tokens.reasoning,
            },
        }
    }

    /// What is on disk, for the Settings tab.
    ///
    /// The saved scan is tens of megabytes in a directory nobody browses, which is exactly
    /// the kind of thing an app should be able to point at and delete rather than leave
    /// for someone to find with a disk usage tool.
    pub fn stored(&self) -> StoredScan {
        let bytes = std::fs::metadata(&self.cache_file)
            .map(|meta| meta.len())
            .unwrap_or(0);
        StoredScan {
            path: self.cache_file.display().to_string(),
            bytes,
            scanned_at: axio_cost::store::load_stamp(&self.cache_file)
                .map(|at| at.to_string()),
            scanning: self.is_scanning(),
        }
    }

    /// Forget the saved scan on disk, so a rescan cannot be short-circuited by it.
    ///
    /// The in-memory report is deliberately left alone: dropping it would blank the table
    /// for the half-minute the rescan takes, and stale figures beat no figures.
    pub fn invalidate(&self) {
        let _ = std::fs::remove_file(&self.cache_file);
    }
}

/// The dearest `limit` rows of a grouping, for a chart rather than a table.
///
/// Truncated deliberately and without a remainder row: these are bars beside a total that
/// is stated in full a few pixels away, and a chart's job is the shape of the top few.
fn top_rows(report: &ScanReport, group: &str, limit: usize) -> Vec<CostRow> {
    let mut rows = group_report(report, group).rows;
    rows.truncate(limit);
    rows
}

fn group_report(report: &ScanReport, group: &str) -> CostReport {
    let prices = Prices::bundled();
    let mut groups: BTreeMap<String, Totals> = BTreeMap::new();
    let mut grand = Totals::default();

    for message in report.messages() {
        groups
            .entry(key_of(message, group))
            .or_default()
            .add(message, &prices);
        grand.add(message, &prices);
    }

    let mut rows: Vec<CostRow> = groups
        .into_iter()
        .map(|(key, totals)| row(key, &totals))
        .collect();
    // By spend, because the row anyone is looking for is the expensive one.
    rows.sort_by(|a, b| {
        b.cost_usd
            .unwrap_or(0.0)
            .partial_cmp(&a.cost_usd.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    CostReport {
        rows,
        total: row("total".into(), &grand),
        unpriced_models: grand.unpriced_models.iter().cloned().collect(),
        agents: report
            .agents
            .iter()
            .map(|agent| AgentRow {
                name: agent.display_name,
                present: agent.present,
                files: agent.files_read,
                messages: agent.messages.len(),
            })
            .collect(),
        loading: false,
        // Set by the caller, which is the only thing that knows whether a worker is up.
        scanning: false,
    }
}

fn row(key: String, totals: &Totals) -> CostRow {
    // `Cost` refuses to hand back a bare number when too little of the row is priced, and
    // that refusal is carried through to the view rather than unwrapped here.
    let (cost_usd, coverage) = match totals.cost() {
        Cost::Complete(dollars) => (Some(dollars), 1.0),
        Cost::Partial { dollars, covered } => (Some(dollars), covered),
        Cost::Unknown => (None, totals.coverage()),
    };
    CostRow {
        key,
        messages: totals.messages,
        tokens: totals.tokens.total(),
        cost_usd,
        coverage,
    }
}

fn key_of(message: &CostMessage, group: &str) -> String {
    match group {
        // The provider is derived from the model, never from the directory the log sits
        // in: a Claude Code transcript here bills OpenAI, DeepSeek and Z.ai as well.
        "provider" => axio_cost::provider_of(&message.model).to_string(),
        "harness" | "client" => message.client.to_string(),
        "session" => message.session_id.clone(),
        "day" => message.timestamp.date().to_string(),
        "workspace" => message
            .workspace
            .clone()
            .unwrap_or_else(|| "(none)".to_string()),
        // Normalized so one model is one row: the logs disagree about spelling, and
        // `claude-haiku-4-5` and `claude-haiku-4-5-20251001` bill identically.
        _ => axio_cost::pricing::normalize(&message.model),
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_cost::{ClientId, TokenBreakdown};
    use time::macros::datetime;

    fn message(model: &str, client: &str) -> CostMessage {
        CostMessage {
            client: ClientId::new(client),
            model: model.into(),
            session_id: "s1".into(),
            workspace: None,
            timestamp: datetime!(2026-08-02 10:00 UTC),
            tokens: TokenBreakdown {
                input: 1_000_000,
                ..Default::default()
            },
            dedup_key: None,
            turn_start: false,
            reported_cost: None,
        }
    }

    /// The regression the user reported as "it freezes": `report` used to scan inline on
    /// the calling thread, which for a Tauri command is the thread that paints. It must
    /// now answer immediately and say it has nothing, whatever the cache file says.
    #[test]
    fn report_never_scans_on_the_calling_thread() {
        let cache = CostCache::new(std::env::temp_dir().join("axio-absent-cache.jsonl"));
        let before = std::time::Instant::now();
        let report = cache.report("model");
        assert!(report.loading, "nothing has been scanned yet");
        assert!(report.rows.is_empty());
        assert!(
            before.elapsed() < std::time::Duration::from_secs(1),
            "a scan would take seconds; this must return at once"
        );
        assert!(cache.stats().days.is_empty());
    }

    /// The other half of the complaint — "it's not really saving". A scan written on one
    /// run has to come back on the next without touching a transcript.
    #[test]
    fn a_saved_scan_comes_back_without_rescanning() {
        let path = std::env::temp_dir().join("axio-warm-cache").join("cost-scan.jsonl");
        let _ = std::fs::remove_file(&path);
        let saved = ScanReport {
            agents: vec![axio_cost::AgentReport {
                client: "codex",
                display_name: "Codex",
                present: true,
                files_read: 2,
                files_failed: 0,
                outcome: Default::default(),
                messages: vec![message("claude-opus-5", "codex")],
            }],
        };
        axio_cost::store::save(&path, &saved).expect("write");

        let cache = CostCache::new(path);
        assert!(cache.warm(), "the saved scan is published");
        let report = cache.report("model");
        assert!(!report.loading);
        assert_eq!(report.total.messages, 1);
        assert_eq!(report.rows[0].key, "claude-opus-5");
        assert!(!cache.warm(), "a second warm must not replace live data");
    }

    fn report_of(messages: Vec<CostMessage>) -> ScanReport {
        ScanReport {
            agents: vec![axio_cost::AgentReport {
                client: "test",
                display_name: "Test",
                present: true,
                files_read: 1,
                files_failed: 0,
                outcome: Default::default(),
                messages,
            }],
        }
    }

    #[test]
    fn rows_are_ordered_by_spend() {
        let scanned = report_of(vec![
            message("claude-haiku-4-5", "a"),
            message("claude-opus-5", "a"),
        ]);
        let report = group_report(&scanned, "model");
        assert_eq!(report.rows[0].key, "claude-opus-5", "$5 before $1");
        assert_eq!(report.rows[1].key, "claude-haiku-4-5");
    }

    /// The view must be able to tell "we could not price this" from "$0.00".
    #[test]
    fn an_unpriced_row_carries_no_number() {
        let scanned = report_of(vec![message("unlisted-model-1", "a")]);
        let report = group_report(&scanned, "model");
        assert_eq!(report.rows[0].cost_usd, None);
        assert_eq!(report.unpriced_models, vec!["unlisted-model-1".to_string()]);
    }

    #[test]
    fn a_partly_priced_total_reports_its_coverage() {
        let scanned = report_of(vec![message("claude-opus-5", "a"), message("unlisted-model-1", "a")]);
        let report = group_report(&scanned, "model");
        assert!((report.total.coverage - 0.5).abs() < 1e-9);
        assert_eq!(report.total.cost_usd, Some(5.0));
    }

    #[test]
    fn one_model_is_one_row_however_the_log_spelled_it() {
        let scanned = report_of(vec![
            message("claude-haiku-4-5", "a"),
            message("claude-haiku-4-5-20251001", "a"),
        ]);
        let report = group_report(&scanned, "model");
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].messages, 2);
    }

    /// The harness is the tool that ran; the provider is the company that charged. They
    /// are different answers, and for a proxied session they are different companies.
    #[test]
    fn harness_and_provider_are_separate_cuts() {
        let scanned = report_of(vec![message("gpt-5.6-sol", "claude-code")]);
        assert_eq!(group_report(&scanned, "harness").rows[0].key, "claude-code");
        assert_eq!(group_report(&scanned, "provider").rows[0].key, "OpenAI");
    }

    #[test]
    fn grouping_switches_without_rescanning() {
        let scanned = report_of(vec![message("claude-opus-5", "codex")]);
        assert_eq!(group_report(&scanned, "harness").rows[0].key, "codex");
        assert_eq!(group_report(&scanned, "day").rows[0].key, "2026-08-02");
        assert_eq!(group_report(&scanned, "workspace").rows[0].key, "(none)");
    }

    #[test]
    fn an_empty_report_says_it_is_loading_rather_than_showing_zero() {
        let empty = CostReport::empty();
        assert!(empty.loading);
        assert_eq!(empty.total.cost_usd, None);
    }
}
