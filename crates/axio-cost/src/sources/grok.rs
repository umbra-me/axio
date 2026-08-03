//! Grok CLI — JSONL under `~/.grok/sessions/<url-encoded-cwd>/<uuid>/updates.jsonl`.
//!
//! Lines are ACP-style envelopes: `{timestamp, method, params}`. Usage rides on
//! `params.update.usage` when `sessionUpdate` closes a prompt.
//!
//! Two things make this the most accurate parser in the crate:
//!
//! * **Grok reports its own cost.** Each `modelUsage` entry carries `costUsdTicks`, the
//!   vendor's arithmetic against the plan the user is actually on. That beats any table
//!   here — no model id to normalize, no rate to go stale, no discount to miss — so it is
//!   used in preference to pricing whenever present.
//! * **`modelUsage` splits the turn by model.** One prompt can bill against
//!   `grok-4.5-build` and `grok-4.5-build-free` at once; the outer figures are their sum.
//!   Emitting one message per entry keeps the per-model view honest.
//!
//! The timestamp is Unix **seconds**, not the RFC 3339 string every other source writes.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::message::{ClientId, CostMessage, DedupLedger};
use crate::tokens::from_inclusive_input;

use super::{FileOutcome, Source};

pub struct Grok;

/// `costUsdTicks` per US dollar.
///
/// Not documented; derived from the data. A turn of 1.54M input and 19K output reports
/// 8,700,136,000 ticks. At this scale that is $8.70, which is the right order of
/// magnitude for the tokens involved; the next scale up would make one turn $8,700 and
/// the next down less than a cent.
const TICKS_PER_DOLLAR: f64 = 1_000_000_000.0;

#[derive(Deserialize)]
struct Line<'a> {
    /// Unix seconds.
    timestamp: Option<i64>,
    #[serde(borrow)]
    params: Option<Params<'a>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Params<'a> {
    #[serde(borrow)]
    session_id: Option<std::borrow::Cow<'a, str>>,
    update: Option<Update>,
}

#[derive(Deserialize)]
struct Update {
    prompt_id: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    /// Per-model breakdown. The outer totals are its sum, so using both would double.
    #[serde(default)]
    model_usage: std::collections::BTreeMap<String, Counts>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct Counts {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    /// A subset of `input_tokens`, as with the Responses dialect — `totalTokens` equals
    /// input plus output, so the cached figure is inside input rather than beside it.
    #[serde(default)]
    cached_read_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
    #[serde(default)]
    cost_usd_ticks: Option<f64>,
}

impl Source for Grok {
    fn client(&self) -> &'static str {
        "grok"
    }

    fn display_name(&self) -> &'static str {
        "Grok"
    }

    fn roots(&self, home: &Path) -> Vec<PathBuf> {
        vec![home.join(".grok").join("sessions")]
    }

    fn owns(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| name == "updates.jsonl")
    }

    fn parse(&self, path: &Path, contents: &str, out: &mut DedupLedger) -> FileOutcome {
        let workspace = workspace_of(path);
        let fallback_session = path
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut outcome = FileOutcome::default();

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(parsed) = serde_json::from_str::<Line>(line) else {
                outcome.malformed += 1;
                continue;
            };
            let Some(usage) = parsed
                .params
                .as_ref()
                .and_then(|params| params.update.as_ref())
                .and_then(|update| update.usage.as_ref())
            else {
                outcome.skipped += 1;
                continue;
            };
            let Some(timestamp) = parsed
                .timestamp
                .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok())
            else {
                outcome.skipped += 1;
                continue;
            };

            let params = parsed.params.as_ref().expect("checked above");
            let session_id = params
                .session_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| fallback_session.clone());
            let prompt_id = params
                .update
                .as_ref()
                .and_then(|update| update.prompt_id.as_deref());

            for (model, counts) in &usage.model_usage {
                let candidate = CostMessage {
                    client: ClientId::new(self.client()),
                    model: model.clone(),
                    session_id: session_id.clone(),
                    workspace: workspace.clone(),
                    timestamp,
                    tokens: from_inclusive_input(
                        counts.input_tokens,
                        counts.cached_read_tokens,
                        counts.output_tokens,
                        counts.reasoning_tokens,
                    ),
                    // The prompt is the unit of work; the model qualifies it because one
                    // prompt can bill against several. Without the model in the key, two
                    // entries of the same prompt would merge into one.
                    dedup_key: prompt_id.map(|id| format!("{session_id}:{id}:{model}")),
                    turn_start: false,
                    reported_cost: counts.cost_usd_ticks.map(|ticks| ticks / TICKS_PER_DOLLAR),
                };

                if candidate.is_billable() {
                    out.push(candidate);
                    outcome.billable += 1;
                } else {
                    outcome.skipped += 1;
                }
            }
        }

        outcome
    }
}

/// The URL-encoded working directory two levels up from `updates.jsonl`.
///
/// Decoded far enough to be readable — `%3A` and `%5C` are the colon and backslash of a
/// Windows path, and a label reading `C%3A%5CUsers` helps nobody.
fn workspace_of(path: &Path) -> Option<String> {
    let encoded = path.parent()?.parent()?.file_name()?.to_str()?;
    Some(
        encoded
            .replace("%3A", ":")
            .replace("%5C", "\\")
            .replace("%2F", "/"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(contents: &str) -> (FileOutcome, Vec<CostMessage>) {
        let mut ledger = DedupLedger::new();
        let path = Path::new(r"/h/.grok/sessions/C%3A%5CUsers%5Cuser/019f7ded/updates.jsonl");
        let outcome = Grok.parse(path, contents, &mut ledger);
        (outcome, ledger.into_messages())
    }

    /// The exact envelope from a real session on this machine.
    const TURN: &str = r#"{"timestamp":1784524526,"method":"session/update","params":{"sessionId":"019f7ded","update":{"sessionUpdate":"prompt_complete","prompt_id":"p1","usage":{"inputTokens":1540373,"outputTokens":19199,"cachedReadTokens":1368192,"reasoningTokens":12492,"modelUsage":{"grok-4.5-build":{"inputTokens":1540373,"outputTokens":19199,"totalTokens":1559572,"cachedReadTokens":1368192,"reasoningTokens":12492,"costUsdTicks":8700136000}}}}}}"#;

    #[test]
    fn the_vendors_own_cost_is_preferred_to_the_table() {
        let (outcome, messages) = parse(TURN);
        assert_eq!(outcome.billable, 1);
        assert_eq!(messages[0].reported_cost, Some(8.700136));
        assert_eq!(messages[0].model, "grok-4.5-build");
    }

    #[test]
    fn input_is_made_cache_exclusive() {
        let (_, messages) = parse(TURN);
        assert_eq!(messages[0].tokens.input, 1_540_373 - 1_368_192);
        assert_eq!(messages[0].tokens.cache_read, 1_368_192);
        assert_eq!(messages[0].tokens.total(), 1_559_572, "matches totalTokens");
    }

    /// One prompt billing two models must produce two rows, not one merged row.
    #[test]
    fn a_prompt_split_across_models_yields_one_message_each() {
        let two = TURN.replace(
            r#""costUsdTicks":8700136000}}"#,
            r#""costUsdTicks":8700136000},"grok-4.5-build-free":{"inputTokens":100,"outputTokens":5,"costUsdTicks":0}}"#,
        );
        let (_, messages) = parse(&two);
        assert_eq!(messages.len(), 2);
        let models: Vec<_> = messages.iter().map(|m| m.model.as_str()).collect();
        assert!(models.contains(&"grok-4.5-build-free"));
    }

    #[test]
    fn the_same_prompt_written_twice_is_billed_once() {
        let (_, messages) = parse(&format!("{TURN}\n{TURN}"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.cache_read, 1_368_192, "not doubled");
    }

    #[test]
    fn lines_without_usage_are_skipped_not_malformed() {
        let mode = r#"{"timestamp":1784524128,"method":"session/update","params":{"sessionId":"019f7ded","update":{"sessionUpdate":"current_mode_update","currentModeId":"plan"}}}"#;
        let (outcome, messages) = parse(mode);
        assert_eq!(messages.len(), 0);
        assert_eq!(outcome.malformed, 0);
        assert_eq!(outcome.skipped, 1);
    }

    #[test]
    fn the_encoded_working_directory_is_decoded_for_display() {
        let (_, messages) = parse(TURN);
        assert_eq!(messages[0].workspace.as_deref(), Some(r"C:\Users\user"));
    }

    /// Only `updates.jsonl` belongs to this parser; the sessions tree holds other files.
    #[test]
    fn only_update_logs_are_claimed() {
        assert!(Grok.owns(Path::new("/x/updates.jsonl")));
        assert!(!Grok.owns(Path::new("/x/meta.jsonl")));
    }
}
