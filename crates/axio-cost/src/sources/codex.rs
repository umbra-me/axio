//! Codex — JSONL rollouts under `~/.codex/sessions/<yyyy>/<mm>/<dd>/rollout-*.jsonl`.
//!
//! Usage arrives on `event_msg` lines whose payload is a `token_count`. Each carries two
//! figures, and choosing between them is the whole correctness story of this parser:
//!
//! * `total_token_usage` — **cumulative for the session**, growing with every turn.
//!   Summing these across events multiplies a session by its own turn count.
//! * `last_token_usage` — the usage of the most recent request. This is what to sum.
//!
//! With one caveat established by measurement rather than assumption: the same
//! `last_token_usage` is sometimes written twice in a row, so consecutive repeats must be
//! suppressed. Across the 396 sessions on this machine that carry usage, summing the
//! deduplicated deltas reproduces the session's own final `total_token_usage`
//! **exactly in 390, and within 1% in 4 more**. Without the suppression the first file
//! tested overshot by 25,796 input tokens.
//!
//! Two further conventions, both settled by the provider's own arithmetic
//! (`total_tokens == input_tokens + output_tokens`):
//! `input_tokens` **includes** `cached_input_tokens`, and `reasoning_output_tokens` is
//! **inside** `output_tokens`. See [`crate::tokens`].

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::message::{ClientId, CostMessage, DedupLedger};
use crate::tokens::{TokenBreakdown, from_inclusive_input};

use super::{FileOutcome, Source};

pub struct Codex;

#[derive(Deserialize)]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<&'a str>,
    timestamp: Option<&'a str>,
    payload: Option<Payload<'a>>,
}

#[derive(Deserialize)]
struct Payload<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<&'a str>,
    /// Present on `turn_context` and `session_meta`, which is where the model is named.
    ///
    /// `Cow` rather than `&str`: serde can only borrow a JSON string that needs no
    /// unescaping, and `cwd` is a Windows path — `"D:\\evade.fail-suite"` contains an
    /// escape, so a borrowed field fails the whole line. Losing `turn_context` that way
    /// is silent and total: the model is never learned, and every usage event afterwards
    /// is discarded for having no model to attribute to.
    model: Option<std::borrow::Cow<'a, str>>,
    cwd: Option<std::borrow::Cow<'a, str>>,
    info: Option<Info>,
}

#[derive(Deserialize)]
struct Info {
    last_token_usage: Option<Counts>,
}

#[derive(Deserialize, PartialEq, Eq, Clone, Copy, Default)]
struct Counts {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
}

impl Counts {
    fn breakdown(&self) -> TokenBreakdown {
        from_inclusive_input(
            self.input_tokens,
            self.cached_input_tokens,
            self.output_tokens,
            self.reasoning_output_tokens,
        )
    }
}

impl Source for Codex {
    fn client(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn roots(&self, home: &Path) -> Vec<PathBuf> {
        vec![home.join(".codex").join("sessions")]
    }

    fn parse(&self, path: &Path, contents: &str, out: &mut DedupLedger) -> FileOutcome {
        let session_id = session_id_of(path);
        let mut outcome = FileOutcome::default();

        // Both are streaming state: a session can change model mid-run, and each
        // `turn_context` re-states the model in force from that point on.
        let mut model: Option<String> = None;
        let mut workspace: Option<String> = None;
        let mut previous: Option<Counts> = None;

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(parsed) = serde_json::from_str::<Line>(line) else {
                outcome.malformed += 1;
                continue;
            };
            let Some(payload) = parsed.payload else {
                outcome.skipped += 1;
                continue;
            };

            if matches!(parsed.kind, Some("turn_context" | "session_meta")) {
                if let Some(named) = payload.model.filter(|model| !model.is_empty()) {
                    model = Some(named.to_string());
                }
                if let Some(cwd) = payload.cwd.filter(|cwd| !cwd.is_empty()) {
                    workspace = Some(cwd.to_string());
                }
                outcome.skipped += 1;
                continue;
            }

            if payload.kind != Some("token_count") {
                outcome.skipped += 1;
                continue;
            }

            // A `token_count` with no `info` carries only rate limits — the quota probe's
            // territory, not this crate's.
            let Some(counts) = payload.info.and_then(|info| info.last_token_usage) else {
                outcome.skipped += 1;
                continue;
            };

            if previous == Some(counts) {
                outcome.skipped += 1;
                continue;
            }
            previous = Some(counts);

            let Some(timestamp) = parsed.timestamp.and_then(parse_timestamp) else {
                outcome.skipped += 1;
                continue;
            };

            let candidate = CostMessage {
                client: ClientId::new(self.client()),
                model: model.clone().unwrap_or_default(),
                session_id: session_id.clone(),
                workspace: workspace.clone(),
                timestamp,
                tokens: counts.breakdown(),
                // Codex writes no per-request identifier, so there is nothing to key on.
                // Repeats are handled by the consecutive-suppression above instead.
                dedup_key: None,
                turn_start: false,
                reported_cost: None,
            };

            if candidate.is_billable() {
                out.push(candidate);
                outcome.billable += 1;
            } else {
                outcome.skipped += 1;
            }
        }

        outcome
    }
}

/// The UUID tail of `rollout-<timestamp>-<uuid>.jsonl`.
///
/// The timestamp is already in the file's directory path, so the uuid alone identifies
/// the session and keeps the id short enough to group by in a table.
///
/// The timestamp is itself dash-separated — `2026-04-26T17-34-45` — so splitting on the
/// first dash yields most of the date rather than the id. Five leading segments belong to
/// the timestamp; everything after them is the uuid.
fn session_id_of(path: &Path) -> String {
    const TIMESTAMP_SEGMENTS: usize = 5;

    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown");
    stem.strip_prefix("rollout-")
        .and_then(|rest| {
            rest.splitn(TIMESTAMP_SEGMENTS + 1, '-')
                .nth(TIMESTAMP_SEGMENTS)
        })
        .unwrap_or(stem)
        .to_string()
}

fn parse_timestamp(raw: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(contents: &str) -> (FileOutcome, Vec<CostMessage>) {
        let mut ledger = DedupLedger::new();
        let path =
            Path::new("/h/.codex/sessions/2026/04/26/rollout-2026-04-26T17-34-45-019dc924.jsonl");
        let outcome = Codex.parse(path, contents, &mut ledger);
        (outcome, ledger.into_messages())
    }

    fn token_count(input: u64, cached: u64, output: u64, reasoning: u64) -> String {
        format!(
            r#"{{"timestamp":"2026-04-26T09:35:05.081Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":{reasoning}}}}}}}}}"#
        )
    }

    const CONTEXT: &str = r#"{"timestamp":"2026-04-26T09:34:58.000Z","type":"turn_context","payload":{"cwd":"D:\\evade.fail-suite","model":"gpt-5.5"}}"#;

    /// The exact counts from a real turn. Trusting `input_tokens` as billable would
    /// charge for 71,375 fresh tokens rather than 3,919.
    #[test]
    fn input_is_made_cache_exclusive() {
        let (_, messages) = parse(&format!(
            "{CONTEXT}\n{}",
            token_count(71_375, 67_456, 569, 285)
        ));
        assert_eq!(messages[0].tokens.input, 3_919);
        assert_eq!(messages[0].tokens.cache_read, 67_456);
        assert_eq!(messages[0].tokens.total(), 71_944, "matches total_tokens");
    }

    /// The measured behaviour: the same delta written twice in a row must bill once.
    /// Without this, one real session overshot its own total by 25,796 input tokens.
    #[test]
    fn a_repeated_delta_is_counted_once() {
        let repeated = token_count(1_000, 0, 10, 0);
        let (outcome, messages) = parse(&format!("{CONTEXT}\n{repeated}\n{repeated}"));
        assert_eq!(messages.len(), 1);
        assert_eq!(outcome.billable, 1);
    }

    /// Only *consecutive* repeats are suppressed. An identical delta that recurs after a
    /// different one is a real second request that happened to cost the same.
    #[test]
    fn an_identical_delta_after_a_different_one_still_counts() {
        let a = token_count(1_000, 0, 10, 0);
        let b = token_count(2_000, 0, 20, 0);
        let (_, messages) = parse(&format!("{CONTEXT}\n{a}\n{b}\n{a}"));
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn the_model_comes_from_turn_context_and_can_change_mid_session() {
        let switched = CONTEXT.replace("gpt-5.5", "gpt-5.6-sol");
        let contents = format!(
            "{CONTEXT}\n{}\n{switched}\n{}",
            token_count(100, 0, 1, 0),
            token_count(200, 0, 2, 0)
        );
        let (_, messages) = parse(&contents);
        assert_eq!(messages[0].model, "gpt-5.5");
        assert_eq!(messages[1].model, "gpt-5.6-sol");
    }

    /// A `token_count` before any `turn_context` has no model to attribute to. It is not
    /// billable — guessing a model would invent a price.
    #[test]
    fn usage_with_no_model_yet_is_not_billed() {
        let (outcome, messages) = parse(&token_count(100, 0, 1, 0));
        assert_eq!(messages.len(), 0);
        assert_eq!(outcome.skipped, 1);
    }

    /// The rate-limit-only variant, which appears before the first real turn.
    #[test]
    fn a_token_count_without_info_is_skipped() {
        let line = r#"{"timestamp":"2026-04-26T09:34:58.901Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":19.0}}}}"#;
        let (outcome, messages) = parse(&format!("{CONTEXT}\n{line}"));
        assert_eq!(messages.len(), 0);
        assert_eq!(outcome.malformed, 0);
    }

    #[test]
    fn the_session_id_is_the_uuid_tail_of_the_filename() {
        let path = Path::new("rollout-2026-04-26T17-34-45-019dc924-30bf-7182.jsonl");
        assert_eq!(session_id_of(path), "019dc924-30bf-7182");
    }

    #[test]
    fn the_working_directory_becomes_the_workspace() {
        let (_, messages) = parse(&format!("{CONTEXT}\n{}", token_count(100, 0, 1, 0)));
        assert_eq!(
            messages[0].workspace.as_deref(),
            Some(r"D:\evade.fail-suite")
        );
    }
}
