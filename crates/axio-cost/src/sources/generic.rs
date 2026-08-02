//! One parser, driven by a table, for the agents that differ only in where they put things.
//!
//! Most coding agents log the same event — an assistant turn with a usage object, a model
//! name and a timestamp — and differ only in the directory, the file extension, and how
//! deep in the JSON the usage sits. Writing twenty near-identical modules would mean
//! twenty places for the same bug.
//!
//! So this walks the JSON rather than modelling it. For each document it finds the first
//! object under a usage-shaped key, then the nearest model and timestamp, wherever the
//! agent chose to put them. That is looser than a typed struct per agent, and deliberately
//! so: these formats change without notice, and a parser that hunts for a usage object
//! survives a reshuffle that a struct with a fixed path does not.
//!
//! The cost of looseness is that a *wrong* guess is silent, so every rule here is
//! conservative — an unrecognised document yields nothing rather than something plausible,
//! and [`super::FileOutcome`] counts what was skipped so `--diagnose` can show it.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::message::{ClientId, CostMessage, DedupLedger};

use super::usage::{AnyUsage, Convention};
use super::{FileOutcome, Source};

/// Which files in a root belong to this agent.
#[derive(Debug, Clone, Copy)]
pub enum Matcher {
    /// Any file with this extension.
    Extension(&'static str),
    /// Only files with exactly this name.
    FileName(&'static str),
}

/// An agent that logs JSON, described rather than coded.
///
/// `Copy` because every field is: the catalog is a `const` slice, and the registry needs
/// owned values to box.
#[derive(Debug, Clone, Copy)]
pub struct GenericAgent {
    pub client: &'static str,
    pub display_name: &'static str,
    /// Home-relative directories, `/`-separated. Joined per component so the separator
    /// is the platform's, not the table's.
    pub roots: &'static [&'static str],
    pub matcher: Matcher,
    pub convention: Convention,
}

/// Keys whose value is a usage object.
pub(super) const USAGE_KEYS: &[&str] = &[
    "usage",
    "tokens",
    "tokenUsage",
    "token_usage",
    "usageMetadata",
    "tokensUsed",
    "metrics",
];

/// Keys whose value names the model.
pub(super) const MODEL_KEYS: &[&str] = &["model", "modelId", "model_id", "modelName", "model_slug"];

/// Keys whose value is when the turn happened.
const TIME_KEYS: &[&str] = &[
    "timestamp",
    "time",
    "createdAt",
    "created_at",
    "ts",
    "date",
    "startTime",
];

/// Keys that identify the underlying API call.
pub(super) const ID_KEYS: &[&str] = &["requestId", "request_id", "messageId", "message_id", "id", "uuid"];

impl Source for GenericAgent {
    fn client(&self) -> &'static str {
        self.client
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn roots(&self, home: &Path) -> Vec<PathBuf> {
        self.roots
            .iter()
            .map(|root| root.split('/').fold(home.to_path_buf(), |path, part| path.join(part)))
            .collect()
    }

    fn owns(&self, path: &Path) -> bool {
        match self.matcher {
            Matcher::Extension(extension) => {
                path.extension().is_some_and(|found| found == extension)
            }
            Matcher::FileName(name) => path.file_name().is_some_and(|found| found == name),
        }
    }

    fn parse(&self, path: &Path, contents: &str, out: &mut DedupLedger) -> FileOutcome {
        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string();
        let workspace = path
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string);

        let mut outcome = FileOutcome::default();
        let mut consume = |document: &Value, outcome: &mut FileOutcome| {
            match self.message(document, &session_id, workspace.clone()) {
                Some(message) => {
                    out.push(message);
                    outcome.billable += 1;
                }
                None => outcome.skipped += 1,
            }
        };

        // A whole-file JSON document first: several agents write one array or object per
        // session rather than a line per turn, and such a file is one long "line".
        if let Ok(document) = serde_json::from_str::<Value>(contents.trim()) {
            match &document {
                Value::Array(entries) => {
                    for entry in entries {
                        consume(entry, &mut outcome);
                    }
                }
                _ => consume(&document, &mut outcome),
            }
            return outcome;
        }

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(document) => consume(&document, &mut outcome),
                Err(_) => outcome.malformed += 1,
            }
        }
        outcome
    }
}

impl GenericAgent {
    pub(super) fn message(
        &self,
        document: &Value,
        session_id: &str,
        workspace: Option<String>,
    ) -> Option<CostMessage> {
        let raw = find_usage(document)?;
        let usage: AnyUsage = serde_json::from_value(raw.clone()).ok()?;
        if usage.is_empty() {
            return None;
        }

        let model = find_string(document, MODEL_KEYS)?;
        let timestamp = find_time(document)?;

        let candidate = CostMessage {
            client: ClientId::new(self.client),
            model,
            session_id: session_id.to_string(),
            workspace,
            timestamp,
            tokens: usage.breakdown(self.convention),
            dedup_key: find_string(document, ID_KEYS).map(|id| format!("{session_id}:{id}")),
            turn_start: false,
            reported_cost: None,
        };
        candidate.is_billable().then_some(candidate)
    }
}

/// The first usage-shaped object anywhere in the document.
///
/// Breadth-first, so a usage object at the top of a turn is preferred to one buried in a
/// nested sub-agent record. Depth-first would find whichever branch happened to sort
/// first, which is not a property any of these formats guarantee.
pub(super) fn find_usage(document: &Value) -> Option<&Value> {
    let mut queue = std::collections::VecDeque::from([document]);
    while let Some(value) = queue.pop_front() {
        match value {
            Value::Object(map) => {
                for key in USAGE_KEYS {
                    if let Some(found @ Value::Object(_)) = map.get(*key) {
                        return Some(found);
                    }
                }
                queue.extend(map.values());
            }
            Value::Array(entries) => queue.extend(entries.iter()),
            _ => {}
        }
    }
    None
}

/// The first non-empty string under any of `keys`, searched breadth-first.
///
/// Objects are also accepted: several agents write `model: {"id": "..."}` rather than a
/// bare string, and taking the `id` out of one is the difference between attributing the
/// turn and dropping it.
pub(super) fn find_string(document: &Value, keys: &[&str]) -> Option<String> {
    let mut queue = std::collections::VecDeque::from([document]);
    while let Some(value) = queue.pop_front() {
        match value {
            Value::Object(map) => {
                for key in keys {
                    match map.get(*key) {
                        Some(Value::String(found)) if !found.is_empty() => {
                            return Some(found.clone());
                        }
                        Some(Value::Object(nested)) => {
                            for inner in ["id", "modelId", "name"] {
                                if let Some(Value::String(found)) = nested.get(inner)
                                    && !found.is_empty()
                                {
                                    return Some(found.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                queue.extend(map.values());
            }
            Value::Array(entries) => queue.extend(entries.iter()),
            _ => {}
        }
    }
    None
}

/// A timestamp in any of the three shapes these logs use.
pub(super) fn find_time(document: &Value) -> Option<time::OffsetDateTime> {
    let mut queue = std::collections::VecDeque::from([document]);
    while let Some(value) = queue.pop_front() {
        match value {
            Value::Object(map) => {
                for key in TIME_KEYS {
                    match map.get(*key) {
                        Some(Value::String(text)) => {
                            if let Ok(parsed) = time::OffsetDateTime::parse(
                                text,
                                &time::format_description::well_known::Rfc3339,
                            ) {
                                return Some(parsed);
                            }
                        }
                        Some(Value::Number(number)) => {
                            if let Some(parsed) = number.as_i64().and_then(from_epoch) {
                                return Some(parsed);
                            }
                        }
                        _ => {}
                    }
                }
                queue.extend(map.values());
            }
            Value::Array(entries) => queue.extend(entries.iter()),
            _ => {}
        }
    }
    None
}

/// Unix seconds or milliseconds, told apart by magnitude.
///
/// The boundary is the year 2001 in milliseconds, which is also the year 33658 in
/// seconds. No session log is from either, so the ambiguity is theoretical.
fn from_epoch(value: i64) -> Option<time::OffsetDateTime> {
    const MILLISECONDS_FROM: i64 = 1_000_000_000_000;
    if value.abs() >= MILLISECONDS_FROM {
        time::OffsetDateTime::from_unix_timestamp_nanos(value as i128 * 1_000_000).ok()
    } else {
        time::OffsetDateTime::from_unix_timestamp(value).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: GenericAgent = GenericAgent {
        client: "example",
        display_name: "Example",
        roots: &[".example/sessions"],
        matcher: Matcher::Extension("jsonl"),
        convention: Convention::BySpelling,
    };

    fn parse(contents: &str) -> (FileOutcome, Vec<CostMessage>) {
        let mut ledger = DedupLedger::new();
        let outcome = AGENT.parse(Path::new("/h/.example/sessions/proj/s1.jsonl"), contents, &mut ledger);
        (outcome, ledger.into_messages())
    }

    #[test]
    fn usage_is_found_however_deeply_it_is_nested() {
        let line = r#"{"timestamp":"2026-08-02T10:00:00Z","payload":{"response":{"model":"m1","usage":{"input_tokens":10,"output_tokens":2}}}}"#;
        let (outcome, messages) = parse(line);
        assert_eq!(outcome.billable, 1);
        assert_eq!(messages[0].model, "m1");
        assert_eq!(messages[0].tokens.input, 10);
    }

    #[test]
    fn a_model_written_as_an_object_still_resolves() {
        let line = r#"{"ts":1784524526,"model":{"id":"kimi-k3"},"tokens":{"inputTokens":5,"outputTokens":1}}"#;
        let (_, messages) = parse(line);
        assert_eq!(messages[0].model, "kimi-k3");
    }

    #[test]
    fn unix_seconds_and_milliseconds_are_both_understood() {
        let seconds = r#"{"ts":1784524526,"model":"m","usage":{"input_tokens":1}}"#;
        let millis = r#"{"ts":1784524526000,"model":"m","usage":{"input_tokens":1}}"#;
        let (_, a) = parse(seconds);
        let (_, b) = parse(millis);
        assert_eq!(a[0].timestamp.unix_timestamp(), 1_784_524_526);
        assert_eq!(b[0].timestamp.unix_timestamp(), 1_784_524_526);
    }

    /// Several agents write one JSON array per session rather than a line per turn.
    #[test]
    fn a_whole_file_array_is_read_as_a_list_of_turns() {
        let doc = r#"[{"time":"2026-08-02T10:00:00Z","model":"m","usage":{"input_tokens":3}},
                      {"time":"2026-08-02T10:01:00Z","model":"m","usage":{"input_tokens":4}}]"#;
        let (outcome, messages) = parse(doc);
        assert_eq!(outcome.billable, 2);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn a_document_with_no_usage_is_skipped_not_counted_as_broken() {
        let line = r#"{"timestamp":"2026-08-02T10:00:00Z","type":"user","text":"hello"}"#;
        let (outcome, messages) = parse(line);
        assert_eq!(messages.len(), 0);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.malformed, 0);
    }

    /// A usage object with no model cannot be attributed, and inventing one would invent
    /// a price with it.
    #[test]
    fn usage_without_a_model_is_dropped() {
        let line = r#"{"timestamp":"2026-08-02T10:00:00Z","usage":{"input_tokens":10}}"#;
        let (_, messages) = parse(line);
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn an_identified_call_written_twice_is_billed_once() {
        let line = r#"{"timestamp":"2026-08-02T10:00:00Z","id":"r1","model":"m","usage":{"input_tokens":10,"output_tokens":1}}"#;
        let (_, messages) = parse(&format!("{line}\n{line}"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.input, 10);
    }

    #[test]
    fn the_matcher_decides_which_files_are_claimed() {
        assert!(AGENT.owns(Path::new("/x/a.jsonl")));
        assert!(!AGENT.owns(Path::new("/x/a.json")));

        const BY_NAME: GenericAgent = GenericAgent {
            matcher: Matcher::FileName("events.jsonl"),
            ..AGENT
        };
        assert!(BY_NAME.owns(Path::new("/x/events.jsonl")));
        assert!(!BY_NAME.owns(Path::new("/x/other.jsonl")));
    }

    #[test]
    fn roots_are_joined_per_component_not_by_a_literal_separator() {
        let roots = AGENT.roots(Path::new("/home/u"));
        assert_eq!(roots.len(), 1);
        assert!(roots[0].ends_with("sessions"));
        assert!(roots[0].parent().is_some_and(|dir| dir.ends_with(".example")));
    }
}
