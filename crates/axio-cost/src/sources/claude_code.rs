//! Claude Code — JSONL transcripts under `~/.claude/projects/<encoded-cwd>/<session>.jsonl`.
//!
//! One JSON object per line. Only `type: "assistant"` lines carry usage; user turns, tool
//! results and metadata lines are counted as skipped rather than treated as errors.
//!
//! Two things about this format are easy to get wrong, and both are visible in real
//! transcripts on this machine rather than in any schema:
//!
//! * **The client is not the provider.** A `.claude` transcript here contains
//!   `gpt-5.6-terra`, `deepseek-v4-flash` and `glm-5.2` next to `claude-opus-5`, because
//!   the CLI was pointed at a proxy. Cost is attributed by model, never by directory.
//! * **`requestId` is often null.** It is present on direct Anthropic calls and absent on
//!   proxied ones, so the deduplication key falls back to the message id alone rather
//!   than collapsing every null-request line into one.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::message::{ClientId, CostMessage, DedupLedger};
use crate::tokens::TokenBreakdown;

use super::{FileOutcome, Source};

pub struct ClaudeCode;

#[derive(Deserialize)]
struct Entry<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<&'a str>,
    timestamp: Option<&'a str>,
    #[serde(rename = "requestId")]
    request_id: Option<&'a str>,
    #[serde(rename = "sessionId")]
    session_id: Option<&'a str>,
    message: Option<Message<'a>>,
}

#[derive(Deserialize)]
struct Message<'a> {
    #[serde(borrow)]
    model: Option<&'a str>,
    id: Option<&'a str>,
    usage: Option<Usage>,
}

/// Anthropic reports input *exclusive* of cached reads, so unlike the Responses dialect
/// these fields map straight across with no subtraction.
#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    /// The per-lifetime split of `cache_creation_input_tokens`, when the client writes it.
    #[serde(default)]
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

impl Usage {
    fn breakdown(&self) -> TokenBreakdown {
        // A 1-hour cache write costs 2x the input rate against 1.25x for a 5-minute one,
        // so the split is worth honouring where it exists. Where it does not, the whole
        // figure is attributed to the 5-minute rate: that is the default TTL, and
        // guessing the dearer one would overstate the bill.
        let (write_5m, write_1h) = match &self.cache_creation {
            Some(split)
                if split.ephemeral_5m_input_tokens + split.ephemeral_1h_input_tokens > 0 =>
            {
                (
                    split.ephemeral_5m_input_tokens,
                    split.ephemeral_1h_input_tokens,
                )
            }
            _ => (self.cache_creation_input_tokens, 0),
        };

        TokenBreakdown {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_read: self.cache_read_input_tokens,
            cache_write_5m: write_5m,
            cache_write_1h: write_1h,
            reasoning: 0,
        }
    }
}

impl Source for ClaudeCode {
    fn client(&self) -> &'static str {
        "claude-code"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn roots(&self, home: &Path) -> Vec<PathBuf> {
        vec![home.join(".claude").join("projects")]
    }

    fn parse(&self, path: &Path, contents: &str, out: &mut DedupLedger) -> FileOutcome {
        let workspace = workspace_of(path);
        let fallback_session = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut outcome = FileOutcome::default();

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<Entry>(line) else {
                outcome.malformed += 1;
                continue;
            };
            if entry.kind != Some("assistant") {
                outcome.skipped += 1;
                continue;
            }
            let Some(message) = entry.message else {
                outcome.skipped += 1;
                continue;
            };
            let Some(usage) = message.usage else {
                outcome.skipped += 1;
                continue;
            };
            let Some(timestamp) = entry.timestamp.and_then(parse_timestamp) else {
                outcome.skipped += 1;
                continue;
            };

            let candidate = CostMessage {
                client: ClientId::new(self.client()),
                model: message.model.unwrap_or_default().to_string(),
                session_id: entry
                    .session_id
                    .map(str::to_string)
                    .unwrap_or_else(|| fallback_session.clone()),
                workspace: workspace.clone(),
                timestamp,
                tokens: usage.breakdown(),
                dedup_key: dedup_key(message.id, entry.request_id),
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

/// `msgid:requestid` when both are present, the message id alone when only it is.
///
/// Returning `None` when there is no message id is deliberate: without an identity the
/// message cannot be recognised as a repeat, and treating every keyless line as the same
/// key would collapse a whole file into one row.
fn dedup_key(id: Option<&str>, request: Option<&str>) -> Option<String> {
    let id = id.filter(|id| !id.is_empty())?;
    Some(match request.filter(|request| !request.is_empty()) {
        Some(request) => format!("{id}:{request}"),
        None => id.to_string(),
    })
}

/// The project directory's name, which Claude Code derives from the working directory.
///
/// Kept verbatim rather than decoded back into a path. The encoding replaces both
/// separators and the drive colon with the same `-`, so `C--Users-user` cannot be turned
/// back into `C:\Users\user` without guessing which dashes were which.
fn workspace_of(path: &Path) -> Option<String> {
    path.parent()?
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

fn parse_timestamp(raw: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> (FileOutcome, Vec<CostMessage>) {
        let mut ledger = DedupLedger::new();
        let path = Path::new("/home/u/.claude/projects/C--Users-user/sess.jsonl");
        let outcome = ClaudeCode.parse(path, line, &mut ledger);
        (outcome, ledger.into_messages())
    }

    /// Shape copied from a real transcript on this machine, including the null requestId
    /// and the non-Anthropic model that a proxied session records.
    const PROXIED: &str = r#"{"type":"assistant","timestamp":"2026-07-19T17:17:22.172Z","requestId":null,"sessionId":"035507ba","message":{"model":"gpt-5.6-sol","id":"resp_0acea","usage":{"input_tokens":1006,"cache_creation_input_tokens":0,"cache_read_input_tokens":20992,"output_tokens":11}}}"#;

    #[test]
    fn a_proxied_line_is_attributed_to_its_model_not_the_directory() {
        let (outcome, messages) = parse(PROXIED);
        assert_eq!(outcome.billable, 1);
        assert_eq!(messages[0].model, "gpt-5.6-sol", "not claude-anything");
        assert_eq!(messages[0].client.as_str(), "claude-code");
        assert_eq!(messages[0].tokens.cache_read, 20_992);
        assert_eq!(
            messages[0].tokens.input, 1_006,
            "anthropic input excludes cache"
        );
    }

    #[test]
    fn a_null_request_id_falls_back_to_the_message_id() {
        assert_eq!(dedup_key(Some("m1"), None), Some("m1".into()));
        assert_eq!(dedup_key(Some("m1"), Some("r1")), Some("m1:r1".into()));
        assert_eq!(dedup_key(None, Some("r1")), None, "no identity, no key");
    }

    /// Two lines that are the same call must bill once. This is the streaming case: the
    /// second write carries the finished output count.
    #[test]
    fn a_repeated_call_is_billed_once() {
        let second = PROXIED.replace(r#""output_tokens":11"#, r#""output_tokens":40"#);
        let (_, messages) = parse(&format!("{PROXIED}\n{second}"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.input, 1_006, "not 2012");
        assert_eq!(messages[0].tokens.output, 40, "the completed count wins");
    }

    #[test]
    fn the_one_hour_cache_split_is_honoured_when_present() {
        let line = r#"{"type":"assistant","timestamp":"2026-08-02T10:00:00Z","sessionId":"s","message":{"model":"claude-opus-5","id":"m","usage":{"cache_creation_input_tokens":545900,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":545900}}}}"#;
        let (_, messages) = parse(line);
        assert_eq!(messages[0].tokens.cache_write_1h, 545_900);
        assert_eq!(messages[0].tokens.cache_write_5m, 0);
    }

    /// Without the split, the cheaper lifetime is assumed — overstating a bill is worse
    /// than understating one when the log simply does not say.
    #[test]
    fn an_absent_split_defaults_to_the_five_minute_rate() {
        let line = r#"{"type":"assistant","timestamp":"2026-08-02T10:00:00Z","sessionId":"s","message":{"model":"claude-opus-5","id":"m","usage":{"cache_creation_input_tokens":1000}}}"#;
        let (_, messages) = parse(line);
        assert_eq!(messages[0].tokens.cache_write_5m, 1_000);
        assert_eq!(messages[0].tokens.cache_write_1h, 0);
    }

    #[test]
    fn synthetic_and_non_assistant_lines_are_skipped_not_billed() {
        let synthetic = r#"{"type":"assistant","timestamp":"2026-08-02T10:00:00Z","sessionId":"s","message":{"model":"<synthetic>","id":"m","usage":{"input_tokens":5}}}"#;
        let user = r#"{"type":"user","timestamp":"2026-08-02T10:00:00Z"}"#;
        let (outcome, messages) = parse(&format!("{synthetic}\n{user}"));
        assert_eq!(messages.len(), 0);
        assert_eq!(outcome.skipped, 2);
        assert_eq!(outcome.malformed, 0);
    }

    /// A transcript being appended to while we read ends in a partial line. That costs
    /// the line, never the rest of the file.
    #[test]
    fn a_truncated_final_line_costs_only_itself() {
        let truncated = &PROXIED[..PROXIED.len() / 2];
        let (outcome, messages) = parse(&format!("{PROXIED}\n{truncated}"));
        assert_eq!(outcome.billable, 1);
        assert_eq!(outcome.malformed, 1);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn the_project_directory_becomes_the_workspace_label() {
        let (_, messages) = parse(PROXIED);
        assert_eq!(messages[0].workspace.as_deref(), Some("C--Users-user"));
    }
}
