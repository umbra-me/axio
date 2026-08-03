//! `axio cost` — what the agents on this machine have spent.
//!
//! Sibling to `axio quota`, and reading a different thing. Quota asks each vendor's API
//! how much of a limit is left; cost reads session transcripts already on disk and adds
//! them up. Neither needs axio's own credentials, and this one opens no socket at all.
//!
//! Every figure printed here carries how much of the underlying volume it accounts for.
//! A total assembled from partly-unpriced data is not a cost, and `axio_cost::Cost` makes
//! that impossible to forget — see its module docs for the mistake that motivated it.

use std::collections::BTreeMap;

use axio_cost::pricing::Prices;

pub(crate) mod calendar;
pub(crate) mod prices;
pub(crate) use prices::import_prices;
use axio_cost::sources::{registry, scan};
use axio_cost::totals::{Totals, render};
use axio_cost::{CostMessage, ScanReport};

/// What to group the table by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum GroupBy {
    /// Which model was billed. The default, because it is what the money follows.
    Model,
    /// Which company charged — derived from the model, not from the directory.
    Provider,
    /// Which agent was run. `client` is accepted as the older name for it.
    #[value(alias = "client")]
    Harness,
    /// One row per session.
    Session,
    /// One row per calendar day, UTC.
    Day,
    /// Project or working directory, where the agent records one.
    Workspace,
}

impl GroupBy {
    fn key(self, message: &CostMessage) -> String {
        match self {
            // Normalized, so one model is one row. The raw strings disagree about
            // spelling — `claude-haiku-4-5` and `claude-haiku-4-5-20251001` are the same
            // billable model, as are `claude-fable-5` and `anthropic/claude-fable-5` —
            // and grouping on them raw splits one model's spend across several lines.
            GroupBy::Model => axio_cost::pricing::normalize(&message.model),
            GroupBy::Provider => axio_cost::provider_of(&message.model).to_string(),
            GroupBy::Harness => message.client.to_string(),
            GroupBy::Session => message.session_id.clone(),
            GroupBy::Day => message.timestamp.date().to_string(),
            GroupBy::Workspace => message
                .workspace
                .clone()
                .unwrap_or_else(|| "(none)".to_string()),
        }
    }

    fn heading(self) -> &'static str {
        match self {
            GroupBy::Model => "model",
            GroupBy::Provider => "provider",
            GroupBy::Harness => "harness",
            GroupBy::Session => "session",
            GroupBy::Day => "day",
            GroupBy::Workspace => "workspace",
        }
    }
}

pub(crate) fn cost_command(
    by: GroupBy,
    json: bool,
    diagnose: bool,
    calendar: bool,
    cached: bool,
    limit: usize,
) -> u8 {
    let Some(home) = home_dir() else {
        eprintln!("axio: no home directory to scan");
        return 1;
    };

    let report = read_scan(&home, cached, json);
    let prices = prices::prices_for(&home);

    if diagnose {
        return diagnose_report(&report, &prices);
    }
    if calendar {
        // Colour is a property of the sink, never of the session — the same rule the
        // agent surfaces follow, so a redirected calendar carries no escape bytes.
        let colour = std::io::IsTerminal::is_terminal(&std::io::stdout())
            && std::env::var_os("NO_COLOR").is_none();
        return calendar::calendar_command(&report, &prices, colour);
    }

    let mut groups: BTreeMap<String, Totals> = BTreeMap::new();
    let mut grand = Totals::default();
    for message in report.messages() {
        groups
            .entry(by.key(message))
            .or_default()
            .add(message, &prices);
        grand.add(message, &prices);
    }

    if json {
        return emit_json(by, &groups, &grand, &report, &prices);
    }

    // Sorted by spend rather than by name: the row someone is looking for is nearly
    // always the expensive one, and an alphabetical table buries it.
    let mut rows: Vec<_> = groups.into_iter().collect();
    rows.sort_by(|a, b| {
        spend(&b.1)
            .partial_cmp(&spend(&a.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    println!(
        "{:<34} {:>10} {:>16}  {}",
        by.heading(),
        "messages",
        "tokens",
        "cost"
    );
    let shown = rows.len().min(limit);
    for (key, totals) in rows.iter().take(shown) {
        println!(
            "{:<34} {:>10} {:>16}  {}",
            truncate(key, 34),
            totals.messages,
            totals.tokens.total(),
            render(&totals.cost()),
        );
    }
    if rows.len() > shown {
        // Never truncate silently — a table that hides rows without saying so reads as
        // the whole picture.
        println!("... {} more rows, use --limit to see them", rows.len() - shown);
    }

    println!(
        "\n{:<34} {:>10} {:>16}  {}",
        "total",
        grand.messages,
        grand.tokens.total(),
        render(&grand.cost()),
    );

    // The two questions a total prompts — who is going to invoice me, and which tool spent
    // it — are different cuts of the same money, and neither is answerable from a table
    // grouped by the other. Printing both under every total saves running this three times.
    if by != GroupBy::Provider {
        summarise("by provider", GroupBy::Provider, report.messages(), &prices);
    }
    if by != GroupBy::Harness {
        summarise("by harness", GroupBy::Harness, report.messages(), &prices);
    }

    if !grand.unpriced_models.is_empty() {
        let names: Vec<_> = grand.unpriced_models.iter().map(String::as_str).collect();
        println!("\nunpriced models: {}", names.join(", "));
    }
    0
}

/// Where the desktop app saves its scan, so the two surfaces share one.
///
/// Machine-local rather than roaming: this describes files on *this* machine and is tens
/// of megabytes of data that can be rebuilt from them in half a minute.
fn cache_path() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local").join("share")))?;
    Some(axio_cost::store::cache_path(&dir.join("axio")))
}

/// Scan the transcripts, or read the saved scan when the caller asked for speed.
///
/// Scanning is the default and stays the default: this command's output is the answer
/// someone quotes, and a cache is by construction a few minutes behind. `--cached` is for
/// the case where that does not matter and thirty seconds does.
///
/// A fresh scan is always written back, whoever asked for it, so the desktop app opens on
/// figures the CLI produced and the reverse.
fn read_scan(home: &std::path::Path, cached: bool, quiet: bool) -> ScanReport {
    let path = cache_path();

    if cached && let Some(saved) = path.as_deref().and_then(axio_cost::store::load) {
        if !quiet {
            // Never silently. A figure from a cache is a different claim from a figure
            // from the transcripts, and the difference is one line to state.
            let minutes = saved.age().whole_minutes().max(0);
            eprintln!("axio: saved scan from {minutes} minutes ago; omit --cached to rescan");
        }
        return saved.report;
    }

    let report = scan(home, &registry());
    if let Some(path) = path {
        let _ = axio_cost::store::save(&path, &report);
    }
    report
}

/// A compact breakdown under the total, so provider and harness are always visible.
fn summarise<'a>(
    heading: &str,
    by: GroupBy,
    messages: impl Iterator<Item = &'a CostMessage>,
    prices: &Prices,
) {
    let mut groups: BTreeMap<String, Totals> = BTreeMap::new();
    for message in messages {
        groups.entry(by.key(message)).or_default().add(message, prices);
    }
    if groups.len() < 2 {
        // One row would only restate the total in different words.
        return;
    }

    let mut rows: Vec<_> = groups.into_iter().collect();
    rows.sort_by(|a, b| {
        spend(&b.1)
            .partial_cmp(&spend(&a.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    println!("
{heading}");
    for (key, totals) in rows {
        println!(
            "  {:<32} {:>10} {:>16}  {}",
            truncate(&key, 32),
            totals.messages,
            totals.tokens.total(),
            render(&totals.cost()),
        );
    }
}

/// What a group cost, for sorting. Unpriced groups sort by tokens instead so they do not
/// all collapse to the bottom as if they were free.
fn spend(totals: &Totals) -> f64 {
    totals
        .cost()
        .partial()
        .map(|(dollars, _)| dollars)
        .unwrap_or(0.0)
}

fn diagnose_report(report: &ScanReport, prices: &Prices) -> u8 {
    for agent in &report.agents {
        if !agent.present {
            println!("{:<14} not installed", agent.display_name);
            continue;
        }
        let mut totals = Totals::default();
        totals.extend(&agent.messages, prices);
        println!(
            "{:<14} {:>5} files  {:>7} billable  {:>5} skipped  {:>4} malformed  {:>4} unreadable  {}",
            agent.display_name,
            agent.files_read,
            agent.outcome.billable,
            agent.outcome.skipped,
            agent.outcome.malformed,
            agent.files_failed,
            render(&totals.cost()),
        );
        if totals.reported_messages > 0 {
            println!(
                "               {} messages costed by the agent's own figure",
                totals.reported_messages
            );
        }
        if !totals.unpriced_models.is_empty() {
            let names: Vec<_> = totals.unpriced_models.iter().map(String::as_str).collect();
            println!("               unpriced: {}", names.join(", "));
        }
    }
    0
}

fn emit_json(
    by: GroupBy,
    groups: &BTreeMap<String, Totals>,
    grand: &Totals,
    report: &ScanReport,
    prices: &Prices,
) -> u8 {
    let breakdown = |by: GroupBy| {
        let mut totals: BTreeMap<String, Totals> = BTreeMap::new();
        for message in report.messages() {
            totals.entry(by.key(message)).or_default().add(message, prices);
        }
        totals
            .into_iter()
            .map(|(key, totals)| {
                serde_json::json!({
                    "key": key,
                    "messages": totals.messages,
                    "tokens": totals.tokens.total(),
                    "costUsd": totals.cost().partial().map(|(dollars, _)| dollars),
                })
            })
            .collect::<Vec<_>>()
    };

    let rows: Vec<_> = groups
        .iter()
        .map(|(key, totals)| {
            let (dollars, covered) = match totals.cost().partial() {
                Some((dollars, covered)) => (Some(dollars), Some(covered)),
                None => (None, None),
            };
            serde_json::json!({
                "key": key,
                "messages": totals.messages,
                "tokens": totals.tokens,
                "costUsd": dollars,
                "priceCoverage": covered,
                "unpricedModels": totals.unpriced_models,
            })
        })
        .collect();

    let document = serde_json::json!({
        "groupedBy": by.heading(),
        "rows": rows,
        "byProvider": breakdown(GroupBy::Provider),
        "byHarness": breakdown(GroupBy::Harness),
        "total": {
            "messages": grand.messages,
            "tokens": grand.tokens,
            "costUsd": grand.cost().partial().map(|(dollars, _)| dollars),
            "priceCoverage": grand.coverage(),
        },
    });
    match serde_json::to_string_pretty(&document) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => {
            eprintln!("axio: {err}");
            1
        }
    }
}

fn truncate(text: &str, width: usize) -> String {
    // Truncated from the left: session ids and directory paths differ at the end, so
    // keeping the head would print thirty identical prefixes.
    if text.chars().count() <= width {
        return text.to_string();
    }
    let tail: String = text
        .chars()
        .skip(text.chars().count().saturating_sub(width - 1))
        .collect();
    format!("…{tail}")
}

pub(super) fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_cost::{ClientId, TokenBreakdown};
    use time::macros::datetime;

    fn message(model: &str, client: &str, session: &str) -> CostMessage {
        CostMessage {
            client: ClientId::new(client),
            model: model.into(),
            session_id: session.into(),
            workspace: None,
            timestamp: datetime!(2026-08-02 10:00 UTC),
            tokens: TokenBreakdown { input: 10, ..Default::default() },
            dedup_key: None,
            turn_start: false,
            reported_cost: None,
        }
    }

    #[test]
    fn grouping_keys_pick_the_right_field() {
        let one = message("claude-opus-5", "claude-code", "s1");
        assert_eq!(GroupBy::Model.key(&one), "claude-opus-5");
        let dated = message("claude-haiku-4-5-20251001", "claude-code", "s1");
        assert_eq!(
            GroupBy::Model.key(&dated),
            "claude-haiku-4-5",
            "one model is one row, however the log spelled it"
        );
        assert_eq!(GroupBy::Harness.key(&one), "claude-code");
        // The harness is the directory the log came from; the provider is who charged.
        // For a proxied model those are different companies.
        assert_eq!(GroupBy::Provider.key(&one), "Anthropic");
        let proxied = message("gpt-5.6-sol", "claude-code", "s1");
        assert_eq!(GroupBy::Harness.key(&proxied), "claude-code");
        assert_eq!(GroupBy::Provider.key(&proxied), "OpenAI");
        assert_eq!(GroupBy::Session.key(&one), "s1");
        assert_eq!(GroupBy::Day.key(&one), "2026-08-02");
        assert_eq!(GroupBy::Workspace.key(&one), "(none)");
    }

    /// Session ids and paths differ at the end, so the head is the useless half.
    #[test]
    fn long_keys_keep_their_tail() {
        assert_eq!(truncate("short", 34), "short");
        let long = "ses_04d0fd11bffeDxiFrAF7D8JVG3xxxxxxxxxxxx";
        let cut = truncate(long, 12);
        assert_eq!(cut.chars().count(), 12);
        assert!(cut.ends_with("xxxxxxxxxxxx".get(..11).unwrap_or("")) || cut.starts_with('…'));
    }
}
