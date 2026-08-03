//! opencode — a SQLite store at `~/.local/share/opencode/opencode.db`.
//!
//! Behind the `sqlite` feature, because reaching this store means compiling SQLite's C
//! source. See the feature's note in `Cargo.toml`.
//!
//! Usage lives in `message.data`, a JSON blob per message, joined to `session` for the
//! model and working directory. Per-message rather than the per-session totals that sit
//! in `session.cost` and friends: the session columns are a correct sum, but they collapse
//! a day's work into one row with one timestamp, which a daily rollup cannot then split.
//!
//! # A third token convention
//!
//! opencode's own `total` proves the arithmetic:
//! `total = input + output + reasoning + cache.read`. So **reasoning is added here, not
//! contained in output** — the opposite of Codex, where `total = input + output` with
//! reasoning inside the output figure.
//!
//! [`crate::tokens::TokenBreakdown`] holds reasoning as a subset of output, so this
//! parser folds it *into* output and keeps a copy in the reporting field. Mapping it
//! straight across would drop it from every total: one observed message carries 9,459
//! reasoning tokens against 171 of output, so the row would have read 98% short.
//!
//! opencode also records its own `cost` per message, which is preferred over the price
//! table for the same reason Grok's is.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::message::{ClientId, CostMessage, DedupLedger};
use crate::tokens::TokenBreakdown;

use super::{FileOutcome, Source};

pub struct OpenCode;

#[derive(Deserialize)]
struct MessageData {
    role: Option<String>,
    tokens: Option<Tokens>,
    cost: Option<f64>,
}

#[derive(Deserialize)]
struct Tokens {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    reasoning: u64,
    #[serde(default)]
    cache: Cache,
}

#[derive(Deserialize, Default)]
struct Cache {
    #[serde(default)]
    read: u64,
    #[serde(default)]
    write: u64,
}

/// `session.model` is JSON: `{"id":"kimi-k3","providerID":"opencode-go","variant":"max"}`.
#[derive(Deserialize)]
struct SessionModel {
    id: Option<String>,
}

impl Source for OpenCode {
    fn client(&self) -> &'static str {
        "opencode"
    }

    fn display_name(&self) -> &'static str {
        "opencode"
    }

    fn roots(&self, home: &Path) -> Vec<PathBuf> {
        vec![
            home.join(".local").join("share").join("opencode"),
            // The XDG variable wins when set, but the default above is what a Windows
            // install uses even though it looks like a Unix path.
            home.join(".config").join("opencode"),
        ]
    }

    fn owns(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| name == "opencode.db")
    }

    fn parse(&self, _path: &Path, _contents: &str, _out: &mut DedupLedger) -> FileOutcome {
        // Unreachable: this source overrides `open` and never receives text.
        FileOutcome::default()
    }

    fn open(&self, path: &Path, out: &mut DedupLedger) -> Option<FileOutcome> {
        // Read-only and immutable. `immutable` also stops SQLite from wanting the
        // sidecar WAL and journal, which matters because opencode may be running right
        // now and holding them — without it, opening a live store fails or, worse,
        // recovers the journal and writes to someone else's database.
        let uri = format!("file:{}?mode=ro&immutable=1", path.display());
        let connection = rusqlite::Connection::open_with_flags(
            &uri,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .ok()?;

        let mut statement = connection
            .prepare(
                "SELECT m.id, m.session_id, m.time_created, m.data, s.model, s.directory \
                 FROM message m JOIN session s ON s.id = m.session_id",
            )
            .ok()?;

        let mut outcome = FileOutcome::default();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .ok()?;

        for row in rows {
            let Ok((id, session_id, created_ms, data, model, directory)) = row else {
                outcome.malformed += 1;
                continue;
            };
            let Ok(parsed) = serde_json::from_str::<MessageData>(&data) else {
                outcome.malformed += 1;
                continue;
            };
            // Only assistant turns carry usage; a user turn names the model and nothing else.
            let (Some("assistant"), Some(tokens)) = (parsed.role.as_deref(), parsed.tokens) else {
                outcome.skipped += 1;
                continue;
            };
            let Ok(timestamp) =
                time::OffsetDateTime::from_unix_timestamp_nanos(created_ms as i128 * 1_000_000)
            else {
                outcome.skipped += 1;
                continue;
            };

            let candidate = CostMessage {
                client: ClientId::new(self.client()),
                model: model.as_deref().and_then(model_id).unwrap_or_default(),
                session_id,
                workspace: directory,
                timestamp,
                tokens: TokenBreakdown {
                    input: tokens.input,
                    // Reasoning folded in — see the module note. It is billed at the
                    // output rate and belongs in the figure the output rate multiplies.
                    output: tokens.output.saturating_add(tokens.reasoning),
                    cache_read: tokens.cache.read,
                    cache_write_5m: tokens.cache.write,
                    cache_write_1h: 0,
                    reasoning: tokens.reasoning,
                },
                // Message ids are unique in the table, so the row identity is the call
                // identity and nothing is ever written twice.
                dedup_key: Some(id),
                turn_start: false,
                reported_cost: parsed.cost,
            };

            if candidate.is_billable() {
                out.push(candidate);
                outcome.billable += 1;
            } else {
                outcome.skipped += 1;
            }
        }

        Some(outcome)
    }
}

/// Pull `id` out of the JSON in `session.model`.
fn model_id(raw: &str) -> Option<String> {
    serde_json::from_str::<SessionModel>(raw)
        .ok()?
        .id
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_is_read_out_of_the_session_json() {
        let raw = r#"{"id":"kimi-k3","providerID":"opencode-go","variant":"max"}"#;
        assert_eq!(model_id(raw).as_deref(), Some("kimi-k3"));
        assert_eq!(model_id("not json"), None);
        assert_eq!(model_id(r#"{"providerID":"x"}"#), None);
    }

    #[test]
    fn only_the_opencode_store_is_claimed() {
        assert!(OpenCode.owns(Path::new("/x/opencode.db")));
        assert!(!OpenCode.owns(Path::new("/x/opencode.db-wal")));
        assert!(!OpenCode.owns(Path::new("/x/auth.json")));
    }

    /// Built from a real row. opencode's own `total` is 18,941 for these figures, which
    /// only balances if reasoning is added rather than contained in output — so the
    /// breakdown must reproduce that number, not 9,482.
    #[test]
    fn reasoning_is_folded_into_output_so_the_total_balances() {
        let data = r#"{"role":"assistant","cost":0.172383,"tokens":{"total":18941,"input":9311,"output":171,"reasoning":9459,"cache":{"write":0,"read":0}}}"#;
        let parsed: MessageData = serde_json::from_str(data).expect("parses");
        let tokens = parsed.tokens.expect("has usage");

        let breakdown = TokenBreakdown {
            input: tokens.input,
            output: tokens.output.saturating_add(tokens.reasoning),
            cache_read: tokens.cache.read,
            cache_write_5m: tokens.cache.write,
            cache_write_1h: 0,
            reasoning: tokens.reasoning,
        };

        assert_eq!(breakdown.total(), 18_941, "matches opencode's own total");
        assert_eq!(breakdown.output, 9_630);
        assert_eq!(breakdown.reasoning, 9_459, "still reported separately");
    }

    /// The second real row, which also exercises the cache-read term of the identity.
    #[test]
    fn cached_reads_are_part_of_the_same_identity() {
        let data = r#"{"role":"assistant","cost":0.0347328,"tokens":{"total":19264,"input":9896,"output":95,"reasoning":57,"cache":{"write":0,"read":9216}}}"#;
        let tokens = serde_json::from_str::<MessageData>(data)
            .expect("parses")
            .tokens
            .expect("has usage");
        let breakdown = TokenBreakdown {
            input: tokens.input,
            output: tokens.output + tokens.reasoning,
            cache_read: tokens.cache.read,
            cache_write_5m: tokens.cache.write,
            cache_write_1h: 0,
            reasoning: tokens.reasoning,
        };
        assert_eq!(breakdown.total(), 19_264);
    }

    #[test]
    fn a_user_turn_carries_no_usage() {
        let data = r#"{"role":"user","model":{"providerID":"opencode-go","modelID":"kimi-k3"}}"#;
        let parsed: MessageData = serde_json::from_str(data).expect("parses");
        assert!(parsed.tokens.is_none());
        assert_eq!(parsed.role.as_deref(), Some("user"));
    }
}
