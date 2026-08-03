//! The Cost view's data, and the cache that makes it usable.
//!
//! Scanning every agent's transcripts takes tens of seconds on a machine with real
//! history — 1,771 files and 18 billion tokens on the one this was written on. A window
//! that re-scanned on every tab click would be unusable, so the scan happens once and the
//! grouping, which is cheap, happens per call.
//!
//! The cache is deliberately dumb: filled on first use, replaced only when the view asks
//! for a refresh. Sessions are append-only and minutes old at worst, and a cost figure
//! that is a few minutes stale is not wrong in any way that matters — whereas a UI that
//! blocks for half a minute is.

use std::collections::BTreeMap;
use std::sync::Mutex;

use axio_cost::pricing::Prices;
use axio_cost::sources::{registry, scan};
use axio_cost::totals::{Cost, Totals};
use axio_cost::{CostMessage, ScanReport};
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
        }
    }
}

/// The scan, held so the window can regroup without paying for it again.
#[derive(Default)]
pub struct CostCache {
    scanned: Mutex<Option<ScanReport>>,
}

impl CostCache {
    /// Group the cached scan, scanning first if nothing is cached yet.
    pub fn report(&self, group: &str) -> CostReport {
        let mut cached = self
            .scanned
            .lock()
            .unwrap_or_else(|err| err.into_inner());

        if cached.is_none() {
            let Some(home) = home_dir() else {
                return CostReport::empty();
            };
            *cached = Some(scan(&home, &registry()));
        }

        let Some(report) = cached.as_ref() else {
            return CostReport::empty();
        };
        group_report(report, group)
    }

    /// Drop the cache so the next call rescans.
    pub fn invalidate(&self) {
        *self.scanned.lock().unwrap_or_else(|err| err.into_inner()) = None;
    }
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
