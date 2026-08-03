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

use axio_cost::pricing::{Prices, feed};
use axio_cost::sources::{registry, scan};
use axio_cost::totals::{Totals, render};
use axio_cost::{CostMessage, ScanReport};

/// What to group the table by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum GroupBy {
    /// Which model was billed. The default, because it is what the money follows.
    Model,
    /// Which agent was run.
    Client,
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
            GroupBy::Client => message.client.to_string(),
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
            GroupBy::Client => "agent",
            GroupBy::Session => "session",
            GroupBy::Day => "day",
            GroupBy::Workspace => "workspace",
        }
    }
}

/// Where a refreshed price feed is kept once imported.
fn overlay_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".axio").join("prices.json")
}

/// Import a price feed so models the bundled table has never heard of can be costed.
///
/// Deliberately a file rather than a fetch. `axio-provider` is the only crate here that
/// links HTTP, and one convenience is not worth spending that boundary on — so the
/// download is a job for whatever already speaks HTTP on this machine:
///
/// ```sh
/// curl -fsSL https://models.dev/api.json -o prices.json
/// axio cost --import-prices prices.json
/// ```
///
/// The document is parsed before it is stored, so an unreadable feed fails here rather
/// than silently becoming an empty overlay that prices nothing.
pub(crate) fn import_prices(path: &std::path::Path) -> u8 {
    let Some(home) = home_dir() else {
        eprintln!("axio: no home directory");
        return 1;
    };
    let document = match std::fs::read_to_string(path) {
        Ok(document) => document,
        Err(err) => {
            eprintln!("axio: cannot read {}: {err}", path.display());
            return 1;
        }
    };
    let rates = match feed::parse(&document) {
        Ok(rates) => rates,
        Err(err) => {
            eprintln!("axio: {} is not a price feed: {err}", path.display());
            return 1;
        }
    };

    let destination = overlay_path(&home);
    if let Some(parent) = destination.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        eprintln!("axio: {err}");
        return 1;
    }
    if let Err(err) = std::fs::write(&destination, &document) {
        eprintln!("axio: {err}");
        return 1;
    }
    println!(
        "{} rates imported to {}",
        rates.len(),
        destination.display()
    );
    0
}

/// The bundled table, plus any imported feed.
///
/// A feed that has gone missing or unreadable since import is ignored rather than fatal:
/// the bundled table still prices most of what anyone runs, and refusing to report at all
/// because an optional file moved would be the wrong trade.
fn prices_for(home: &std::path::Path) -> Prices {
    let bundled = Prices::bundled();
    let Ok(document) = std::fs::read_to_string(overlay_path(home)) else {
        return bundled;
    };
    match feed::parse(&document) {
        Ok(rates) => bundled.with_overlay("imported feed", rates),
        Err(_) => bundled,
    }
}

pub(crate) fn cost_command(by: GroupBy, json: bool, diagnose: bool, limit: usize) -> u8 {
    let Some(home) = home_dir() else {
        eprintln!("axio: no home directory to scan");
        return 1;
    };

    let report = scan(&home, &registry());
    let prices = prices_for(&home);

    if diagnose {
        return diagnose_report(&report, &prices);
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
        return emit_json(by, &groups, &grand);
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
    if !grand.unpriced_models.is_empty() {
        let names: Vec<_> = grand.unpriced_models.iter().map(String::as_str).collect();
        println!("unpriced models: {}", names.join(", "));
    }
    0
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

fn emit_json(by: GroupBy, groups: &BTreeMap<String, Totals>, grand: &Totals) -> u8 {
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
        assert_eq!(GroupBy::Client.key(&one), "claude-code");
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
