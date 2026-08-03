//! A saved scan, so a restart does not pay for one.
//!
//! Reading every agent's transcripts takes tens of seconds on a machine with real history.
//! Nothing about that work changes between runs — the files are append-only and mostly
//! untouched — so paying for it on every launch is the whole cost of not writing the
//! answer down.
//!
//! JSONL, one record per line, for the same reasons the quota history uses it: it appends
//! and streams without holding the document in memory twice, a truncated tail costs the
//! messages after the break rather than the file, and it can be read with any tool.
//!
//! What this is *not* is a source of truth. A cache is loaded to put something on screen
//! immediately and is then replaced by a real scan running behind it. Anything that must
//! be right — the CLI's own output — scans. See [`load`] for why staleness is bounded but
//! never zero.

use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::message::CostMessage;
use crate::sources::{AgentReport, FileOutcome, ScanReport, registry};

/// Bumped when a record's shape changes. A cache written by an older build is discarded
/// rather than migrated: it can be rebuilt from the transcripts in half a minute, and
/// migration code for a throwaway file is a liability with no upside.
const VERSION: u32 = 1;

/// Where a scan is kept, given the directory a caller has chosen for machine-local data.
///
/// The caller picks the directory deliberately. This file is derived, large, and specific
/// to one machine's transcripts — on Windows it belongs under `LOCALAPPDATA` and never in
/// the roaming profile, which would sync tens of megabytes of rebuildable data between
/// machines to no purpose.
pub fn cache_path(dir: &Path) -> PathBuf {
    dir.join("cost-scan.jsonl")
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
enum Record {
    /// Always the first line. A file whose first line is not a head of a version we know
    /// is not ours, and is ignored rather than guessed at.
    Head {
        version: u32,
        #[serde(with = "time::serde::rfc3339")]
        scanned_at: OffsetDateTime,
    },
    /// One per agent, carrying the counts the Cost view's footer shows. Kept separate
    /// from the messages because an agent that is installed and recorded nothing is a
    /// different fact from an agent that is not installed, and neither produces a message.
    Agent {
        client: String,
        present: bool,
        files_read: usize,
        files_failed: usize,
        outcome: FileOutcome,
    },
    Message(CostMessage),
}

/// A scan read back from disk, with when it was taken.
pub struct Cached {
    pub report: ScanReport,
    pub scanned_at: OffsetDateTime,
}

impl Cached {
    /// How long ago this was taken. What counts as too old is the caller's judgement, not
    /// this module's — a window showing a figure while it rescans and a script deciding
    /// whether to scan at all want very different answers.
    pub fn age(&self) -> time::Duration {
        OffsetDateTime::now_utc() - self.scanned_at
    }
}

/// Write a scan out, replacing whatever was there.
///
/// Written to a sibling temp file and renamed, so a crash or a full disk mid-write leaves
/// the previous cache intact rather than a half file that parses as a much cheaper year.
pub fn save(path: &Path, report: &ScanReport) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("jsonl.tmp");

    {
        let mut out = BufWriter::new(std::fs::File::create(&temp)?);
        writeln(
            &mut out,
            &Record::Head {
                version: VERSION,
                scanned_at: OffsetDateTime::now_utc(),
            },
        )?;
        for agent in &report.agents {
            writeln(
                &mut out,
                &Record::Agent {
                    client: agent.client.to_string(),
                    present: agent.present,
                    files_read: agent.files_read,
                    files_failed: agent.files_failed,
                    outcome: agent.outcome,
                },
            )?;
            for message in &agent.messages {
                writeln(&mut out, &Record::Message(message.clone()))?;
            }
        }
        out.flush()?;
    }

    std::fs::rename(&temp, path)
}

fn writeln(out: &mut impl Write, record: &Record) -> std::io::Result<()> {
    let line = serde_json::to_string(record)?;
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")
}

/// When the saved scan was taken, reading only its first line.
///
/// Separate from [`load`] because the answer is in the header and the body is tens of
/// megabytes: a view that only wants to say "saved 4 minutes ago" should not parse a
/// hundred thousand messages to find out.
pub fn load_stamp(path: &Path) -> Option<OffsetDateTime> {
    let file = std::fs::File::open(path).ok()?;
    let line = std::io::BufReader::new(file).lines().next()?.ok()?;
    match serde_json::from_str::<Record>(&line).ok()? {
        Record::Head {
            version,
            scanned_at,
        } if version == VERSION => Some(scanned_at),
        _ => None,
    }
}

/// Read a scan back, or `None` if there is nothing usable there.
///
/// Every failure is the same answer — scan instead — so none of them is worth
/// distinguishing to a caller: a missing file, a version we do not write, a truncated
/// line. The one case handled rather than rejected is a torn tail, where the lines before
/// the break are kept: a cache is already an approximation, and losing the last agent's
/// messages is a smaller lie than showing nothing.
pub fn load(path: &Path) -> Option<Cached> {
    let file = std::fs::File::open(path).ok()?;
    let mut lines = std::io::BufReader::new(file).lines();

    let head = serde_json::from_str::<Record>(&lines.next()?.ok()?).ok()?;
    let Record::Head {
        version,
        scanned_at,
    } = head
    else {
        return None;
    };
    if version != VERSION {
        return None;
    }

    // The registry is what turns a stored client name back into the `&'static str` the
    // report carries, and it is also the check that the agent still exists in this build.
    let known = registry();
    let mut agents: Vec<AgentReport> = Vec::new();

    for line in lines {
        let Ok(line) = line else { break };
        // A truncated final line is expected after a hard kill; stop rather than fail.
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            break;
        };
        match record {
            Record::Head { .. } => break,
            Record::Agent {
                client,
                present,
                files_read,
                files_failed,
                outcome,
            } => {
                let Some(source) = known.iter().find(|source| source.client() == client) else {
                    continue;
                };
                agents.push(AgentReport {
                    client: source.client(),
                    display_name: source.display_name(),
                    present,
                    files_read,
                    files_failed,
                    outcome,
                    messages: Vec::new(),
                });
            }
            Record::Message(message) => {
                // Messages follow their agent's line, so the open one is the last pushed.
                // A message with no agent ahead of it means the file was written by
                // something else and is not worth guessing about.
                let Some(agent) = agents.last_mut() else {
                    break;
                };
                agent.messages.push(message);
            }
        }
    }

    if agents.is_empty() {
        return None;
    }
    Some(Cached {
        report: ScanReport { agents },
        scanned_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ClientId;
    use crate::tokens::TokenBreakdown;
    use time::macros::datetime;

    fn message(client: &str, model: &str) -> CostMessage {
        CostMessage {
            client: ClientId::new(client),
            model: model.into(),
            session_id: "s1".into(),
            workspace: Some("W:\\dev".into()),
            timestamp: datetime!(2026-08-02 10:00 UTC),
            tokens: TokenBreakdown {
                input: 10,
                output: 5,
                ..Default::default()
            },
            dedup_key: None,
            turn_start: false,
            reported_cost: None,
        }
    }

    fn report_of(agents: Vec<(&'static str, Vec<CostMessage>)>) -> ScanReport {
        let known = registry();
        ScanReport {
            agents: agents
                .into_iter()
                .map(|(client, messages)| {
                    let source = known
                        .iter()
                        .find(|source| source.client() == client)
                        .expect("test names a registered agent");
                    AgentReport {
                        client: source.client(),
                        display_name: source.display_name(),
                        present: true,
                        files_read: 3,
                        files_failed: 1,
                        outcome: FileOutcome {
                            billable: messages.len(),
                            skipped: 2,
                            malformed: 0,
                        },
                        messages,
                    }
                })
                .collect(),
        }
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("axio-store-{name}"))
            .join("cost-scan.jsonl")
    }

    #[test]
    fn a_saved_scan_reads_back_whole() {
        let path = temp("roundtrip");
        let _ = std::fs::remove_file(&path);
        let original = report_of(vec![
            (
                "codex",
                vec![message("codex", "gpt-5.6-sol"), message("codex", "gpt-5.5")],
            ),
            ("claude-code", vec![message("claude-code", "claude-opus-5")]),
        ]);

        save(&path, &original).expect("write");
        let cached = load(&path).expect("read");

        assert_eq!(cached.report.agents.len(), 2);
        assert_eq!(cached.report.messages().count(), 3);
        assert_eq!(cached.report.agents[0].files_read, 3);
        assert_eq!(cached.report.agents[0].files_failed, 1);
        assert_eq!(cached.report.agents[0].outcome.skipped, 2);
        assert_eq!(cached.report.agents[1].messages[0].model, "claude-opus-5");
        // The display name is recovered from the registry rather than stored, so it can
        // never drift from what the running build calls the agent.
        assert!(!cached.report.agents[0].display_name.is_empty());
    }

    /// The point of the cache: totals from it must equal totals from the scan it came
    /// from, or every figure in the window is wrong for the first second after launch.
    #[test]
    fn totals_survive_the_round_trip() {
        let path = temp("totals");
        let _ = std::fs::remove_file(&path);
        let original = report_of(vec![(
            "codex",
            vec![
                message("codex", "gpt-5.6-sol"),
                message("codex", "gpt-5.6-sol"),
            ],
        )]);
        save(&path, &original).expect("write");

        let prices = crate::pricing::Prices::bundled();
        let mut before = crate::totals::Totals::default();
        before.extend(&original.agents[0].messages, &prices);
        let mut after = crate::totals::Totals::default();
        after.extend(
            &load(&path).expect("read").report.agents[0].messages,
            &prices,
        );

        assert_eq!(before.tokens.total(), after.tokens.total());
        assert_eq!(before.cost().partial(), after.cost().partial());
    }

    #[test]
    fn a_cache_from_another_version_is_ignored() {
        let path = temp("version");
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        std::fs::write(
            &path,
            "{\"t\":\"head\",\"version\":999,\"scannedAt\":\"2026-08-02T10:00:00Z\"}\n",
        )
        .expect("write");
        assert!(load(&path).is_none());
    }

    /// A hard kill mid-write leaves a torn line. The lines before it are still a year's
    /// worth of data and are worth more than nothing.
    #[test]
    fn a_torn_tail_keeps_what_came_before_it() {
        let path = temp("torn");
        let _ = std::fs::remove_file(&path);
        save(
            &path,
            &report_of(vec![("codex", vec![message("codex", "gpt-5.5")])]),
        )
        .expect("write");

        let mut text = std::fs::read_to_string(&path).expect("read");
        text.push_str("{\"t\":\"message\",\"clie");
        std::fs::write(&path, text).expect("write");

        let cached = load(&path).expect("the whole lines still load");
        assert_eq!(cached.report.messages().count(), 1);
    }

    /// The stamp must be readable without parsing the body, because the body is the
    /// reason the stamp exists.
    #[test]
    fn the_stamp_reads_without_the_body() {
        let path = temp("stamp");
        let _ = std::fs::remove_file(&path);
        save(
            &path,
            &report_of(vec![("codex", vec![message("codex", "gpt-5.5")])]),
        )
        .expect("write");

        let stamp = load_stamp(&path).expect("a stamp");
        let whole = load(&path).expect("the whole file");
        assert_eq!(stamp, whole.scanned_at);
        assert!(load_stamp(&temp("absent")).is_none());
    }

    #[test]
    fn nothing_on_disk_is_not_an_error() {
        assert!(load(&temp("absent")).is_none());
    }
}
