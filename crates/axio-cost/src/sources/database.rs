//! Agents whose session store is a SQLite database, described rather than coded.
//!
//! Behind the `sqlite` feature — see the note in `Cargo.toml` for why a C compile does not
//! belong in a default `cargo install`.
//!
//! Nearly all of these keep a JSON blob in a column: `threads.data`, `gen_metadata.data`,
//! `conversations_v2.value`. So the database part is only *getting to the JSON*, and once
//! there the same walker that reads the file-based agents applies unchanged. A row here is
//! a table name and a column, not a parser.
//!
//! # Schemas move, and a missing table is not an error
//!
//! These are other programs' databases, versioned on their schedule. Every agent lists
//! several candidate queries and the first that runs wins; when none do, the file yields
//! nothing and is counted, never fatal. An upgrade that renames a table should cost one
//! agent's rows until a query is added, not the whole scan.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::message::{ClientId, CostMessage, DedupLedger};

use super::generic::{ID_KEYS, MODEL_KEYS, Matcher, find_string, find_time, find_usage};
use super::usage::{AnyUsage, Convention};
use super::{FileOutcome, Source};

/// An agent that keeps its sessions in SQLite.
#[derive(Debug, Clone, Copy)]
pub struct DatabaseAgent {
    pub client: &'static str,
    pub display_name: &'static str,
    /// Home-relative directories to search, `/`-separated.
    pub roots: &'static [&'static str],
    /// Which files in those directories are databases this agent owns.
    ///
    /// A matcher rather than a fixed name because the two are not the same thing: one
    /// agent keeps a single well-known file, another keeps one database per conversation
    /// named for its uuid, and its well-known name is a *table* inside them.
    pub matcher: Matcher,
    /// Candidate queries, tried in order. Each must select a JSON text column first;
    /// anything after it is ignored, so a query may be written for readability.
    pub queries: &'static [&'static str],
    pub convention: Convention,
}

impl Source for DatabaseAgent {
    fn client(&self) -> &'static str {
        self.client
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn roots(&self, home: &Path) -> Vec<PathBuf> {
        self.roots
            .iter()
            .map(|root| {
                root.split('/')
                    .fold(home.to_path_buf(), |path, part| path.join(part))
            })
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

    fn parse(&self, _path: &Path, _contents: &str, _out: &mut DedupLedger) -> FileOutcome {
        // Unreachable: this source overrides `open` and never receives text.
        FileOutcome::default()
    }

    fn open(&self, path: &Path, out: &mut DedupLedger) -> Option<FileOutcome> {
        // Read-only and immutable. `immutable` also stops SQLite wanting the sidecar WAL
        // and journal, which matters because the agent may be running right now and
        // holding them — without it, opening a live store fails or, worse, recovers the
        // journal and writes to someone else's database.
        let uri = format!("file:{}?mode=ro&immutable=1", path.display());
        let connection = rusqlite::Connection::open_with_flags(
            &uri,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .ok()?;

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
        for query in self.queries {
            let Ok(mut statement) = connection.prepare(query) else {
                // A query that does not compile means this schema version lacks the
                // table. Try the next shape rather than giving up on the agent.
                continue;
            };
            let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) else {
                continue;
            };
            for row in rows {
                let Ok(blob) = row else {
                    outcome.malformed += 1;
                    continue;
                };
                let Ok(document) = serde_json::from_str::<Value>(&blob) else {
                    outcome.malformed += 1;
                    continue;
                };
                self.consume(&document, &session_id, &workspace, out, &mut outcome);
            }
            // The first query that ran is the schema in use; running the rest would
            // double-count the same rows under a different name.
            return Some(outcome);
        }
        Some(outcome)
    }
}

impl DatabaseAgent {
    fn consume(
        &self,
        document: &Value,
        session_id: &str,
        workspace: &Option<String>,
        out: &mut DedupLedger,
        outcome: &mut FileOutcome,
    ) {
        // A blob may be one turn or a whole conversation; both shapes occur.
        let entries: Vec<&Value> = match document {
            Value::Array(entries) => entries.iter().collect(),
            other => vec![other],
        };

        for entry in entries {
            let Some(raw) = find_usage(entry) else {
                outcome.skipped += 1;
                continue;
            };
            let Ok(usage) = serde_json::from_value::<AnyUsage>(raw.clone()) else {
                outcome.skipped += 1;
                continue;
            };
            let (Some(model), Some(timestamp)) =
                (find_string(entry, MODEL_KEYS), find_time(entry))
            else {
                outcome.skipped += 1;
                continue;
            };
            if usage.is_empty() {
                outcome.skipped += 1;
                continue;
            }

            let candidate = CostMessage {
                client: ClientId::new(self.client),
                model,
                session_id: session_id.to_string(),
                workspace: workspace.clone(),
                timestamp,
                tokens: usage.breakdown(self.convention),
                dedup_key: find_string(entry, ID_KEYS).map(|id| format!("{session_id}:{id}")),
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
    }
}

/// Every database-backed agent, in a stable order.
///
/// Queries were taken from the reference implementation named in `NOTICE`. Where an agent
/// stores its blob under more than one schema version, each is listed and the first that
/// compiles is used.
pub const CATALOG: &[DatabaseAgent] = &[
    DatabaseAgent {
        client: "zed",
        display_name: "Zed",
        roots: &[
            ".local/share/zed/threads",
            "Library/Application Support/Zed/threads",
            "AppData/Roaming/Zed/threads",
            "AppData/Roaming/Zed/db",
        ],
        matcher: Matcher::FileName("threads.db"),
        queries: &["SELECT data FROM threads"],
        convention: Convention::BySpelling,
    },
    DatabaseAgent {
        client: "antigravity",
        display_name: "Antigravity",
        roots: &[".gemini/antigravity-cli/conversations"],
        matcher: Matcher::Extension("db"),
        queries: &[
            "SELECT data FROM gen_metadata ORDER BY idx",
            "SELECT data FROM trajectory_metadata_blob",
        ],
        convention: Convention::CacheInclusive,
    },
    DatabaseAgent {
        client: "kiro-desktop",
        display_name: "Kiro (desktop)",
        roots: &[
            "Library/Application Support/Kiro/User/globalStorage/kiro.kiroagent",
            "AppData/Roaming/Kiro/User/globalStorage/kiro.kiroagent",
            ".config/Kiro/User/globalStorage/kiro.kiroagent",
        ],
        matcher: Matcher::FileName("data.sqlite3"),
        queries: &["SELECT value FROM conversations_v2"],
        convention: Convention::BySpelling,
    },
    DatabaseAgent {
        client: "crush",
        display_name: "Crush",
        roots: &[".crush", ".local/share/crush", "AppData/Local/crush"],
        matcher: Matcher::FileName("crush.db"),
        queries: &[
            "SELECT parts FROM messages",
            "SELECT data FROM messages",
        ],
        convention: Convention::BySpelling,
    },
    DatabaseAgent {
        client: "devin",
        display_name: "Devin",
        roots: &[".local/share/devin/cli", "AppData/Local/devin/cli"],
        matcher: Matcher::FileName("sessions.db"),
        queries: &["SELECT data FROM sessions", "SELECT value FROM events"],
        convention: Convention::BySpelling,
    },
    DatabaseAgent {
        client: "goose",
        display_name: "Goose",
        roots: &[".local/share/goose", ".config/goose", "AppData/Roaming/goose"],
        matcher: Matcher::FileName("sessions.db"),
        queries: &["SELECT data FROM messages", "SELECT content FROM messages"],
        convention: Convention::BySpelling,
    },
    DatabaseAgent {
        client: "copilot-desktop",
        display_name: "Copilot (desktop)",
        roots: &[".copilot"],
        matcher: Matcher::FileName("session-store.db"),
        queries: &[
            "SELECT value FROM sessions",
            "SELECT data FROM session_state",
        ],
        convention: Convention::BySpelling,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_are_unique_and_distinct_from_the_file_catalog() {
        let mut ids: Vec<_> = CATALOG.iter().map(|agent| agent.client).collect();
        ids.extend(super::super::catalog::CATALOG.iter().map(|agent| agent.client));
        ids.extend(["claude-code", "codex", "grok"]);
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "the client id is the grouping key");
    }

    #[test]
    fn every_agent_has_a_root_and_at_least_one_query() {
        for agent in CATALOG {
            assert!(!agent.roots.is_empty(), "{} has nowhere to look", agent.client);
            assert!(!agent.queries.is_empty(), "{} has nothing to run", agent.client);
        }
    }

    #[test]
    fn only_the_named_database_is_claimed() {
        let zed = CATALOG.iter().find(|a| a.client == "zed").expect("present");
        assert!(zed.owns(Path::new("/x/threads.db")));
        // The sidecar files SQLite writes beside a live database must not be opened as
        // databases themselves.
        assert!(!zed.owns(Path::new("/x/threads.db-wal")));
        assert!(!zed.owns(Path::new("/x/other.db")));
    }

    /// A database that does not exist, or whose schema has moved on, must cost that agent
    /// and nothing else.
    #[test]
    fn a_missing_database_yields_nothing_rather_than_failing() {
        let zed = CATALOG.iter().find(|a| a.client == "zed").expect("present");
        let mut ledger = DedupLedger::new();
        assert!(zed.open(Path::new("/nonexistent/threads.db"), &mut ledger).is_none());
        assert!(ledger.is_empty());
    }

    #[test]
    fn a_schema_without_the_expected_table_is_survived() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("threads.db");
        let connection = rusqlite::Connection::open(&path).expect("creates");
        connection
            .execute("CREATE TABLE unrelated (x TEXT)", [])
            .expect("creates a table this agent knows nothing about");
        drop(connection);

        let zed = CATALOG.iter().find(|a| a.client == "zed").expect("present");
        let mut ledger = DedupLedger::new();
        let outcome = zed.open(&path, &mut ledger).expect("opens");
        assert_eq!(outcome.billable, 0);
        assert_eq!(outcome.malformed, 0, "a missing table is not a broken row");
    }

    #[test]
    fn a_json_blob_column_is_read_through_the_shared_walker() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("threads.db");
        let connection = rusqlite::Connection::open(&path).expect("creates");
        connection
            .execute("CREATE TABLE threads (data TEXT)", [])
            .expect("creates");
        connection
            .execute(
                "INSERT INTO threads (data) VALUES (?1)",
                [r#"{"timestamp":"2026-08-02T10:00:00Z","model":"m1","usage":{"input_tokens":10,"output_tokens":2}}"#],
            )
            .expect("inserts");
        drop(connection);

        let zed = CATALOG.iter().find(|a| a.client == "zed").expect("present");
        let mut ledger = DedupLedger::new();
        let outcome = zed.open(&path, &mut ledger).expect("opens");
        assert_eq!(outcome.billable, 1);

        let messages = ledger.into_messages();
        assert_eq!(messages[0].model, "m1");
        assert_eq!(messages[0].tokens.input, 10);
        assert_eq!(messages[0].client.as_str(), "zed");
    }

    /// A blob holding a whole conversation must yield one message per turn.
    #[test]
    fn an_array_blob_yields_every_turn_in_it() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("threads.db");
        let connection = rusqlite::Connection::open(&path).expect("creates");
        connection
            .execute("CREATE TABLE threads (data TEXT)", [])
            .expect("creates");
        connection
            .execute(
                "INSERT INTO threads (data) VALUES (?1)",
                [r#"[{"ts":1784524526,"model":"m","usage":{"input_tokens":3}},
                     {"ts":1784524600,"model":"m","usage":{"input_tokens":4}}]"#],
            )
            .expect("inserts");
        drop(connection);

        let zed = CATALOG.iter().find(|a| a.client == "zed").expect("present");
        let mut ledger = DedupLedger::new();
        let outcome = zed.open(&path, &mut ledger).expect("opens");
        assert_eq!(outcome.billable, 2);
    }
}
