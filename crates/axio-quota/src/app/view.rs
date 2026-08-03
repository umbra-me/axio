//! What the Cost and Stats views receive.
//!
//! Split from the cache when the file outgrew the length gate, along the line that was
//! already there: everything here is a wire type the webview deserializes, and nothing
//! here decides anything. Field names are camelCase because that is what TypeScript reads.

use serde::Serialize;

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
    /// Cache reads as a share of this row's tokens, `0.0..=1.0`.
    ///
    /// A share rather than a multiple of fresh input, because the vendors disagree about
    /// what "input" means: measured live, the multiple put gpt-5.6-sol at 31x and
    /// claude-opus-5 at 121,077x, which is one column holding two incomparable numbers.
    pub cache_ratio: Option<f64>,
    /// Dollars per million tokens: what this row actually cost, blended across every rate
    /// its models charge. The one figure that compares a cheap model used heavily against
    /// an expensive one used sparingly.
    pub per_million: Option<f64>,
    /// This row's share of the report's total cost, `0.0..=1.0`.
    pub share: f64,
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
    pub(super) fn empty() -> Self {
        CostReport {
            rows: Vec::new(),
            total: CostRow {
                key: "total".into(),
                messages: 0,
                tokens: 0,
                cost_usd: None,
                coverage: 1.0,
                cache_ratio: None,
                per_million: None,
                share: 0.0,
            },
            unpriced_models: Vec::new(),
            agents: Vec::new(),
            loading: true,
            scanning: true,
        }
    }
}
