//! `axio cost --json`.
//!
//! A separate document from the table rather than a serialization of it: a script wants
//! every row and both breakdowns, where a terminal wants the dearest twenty-five.

use std::collections::BTreeMap;

use axio_cost::ScanReport;
use axio_cost::pricing::Prices;
use axio_cost::totals::Totals;

use super::GroupBy;

pub(super) fn emit_json(
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
