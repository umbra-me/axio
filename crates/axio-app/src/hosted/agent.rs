//! What a hosted agent says about itself, and what it wrote down.
//!
//! Two sources, one shape. A tool with hooks — Claude Code, Codex — reports
//! the events that matter through `crate::hooks`; a tool without them can
//! print an in-band status sequence; both land here as a small state: what
//! it is doing, what its own session is called, and where its transcript
//! is. Nothing here reads the terminal's bytes to guess.

use std::path::PathBuf;

use axio_pty::Harness;

use crate::model::{AppError, TranscriptEntry, TranscriptView};

use super::Hosted;

#[derive(Debug, Clone, Default)]
pub(super) struct AgentState {
    /// `working`, `blocked`, `idle` or `done`.
    pub status: Option<String>,
    /// The tool's own session id.
    pub session: Option<String>,
    /// The tool's own transcript file, when it names one.
    pub transcript: Option<PathBuf>,
}

impl Hosted {
    /// Tell every terminal started from now on where its hooks report.
    pub fn listen_at(&self, endpoint: crate::hooks::HookEndpoint) {
        *self.hooks.lock().expect("no lock is held across an await") = Some(endpoint);
    }

    /// The hook arguments and environment for a terminal, when listening.
    pub(super) fn hook_launch(
        &self,
        harness: Harness,
        id: &str,
    ) -> (Vec<String>, Vec<(String, String)>) {
        match &*self.hooks.lock().expect("no lock is held across an await") {
            Some(endpoint) => (
                harness.hook_args(&endpoint.script.to_string_lossy()),
                endpoint.env(id),
            ),
            None => (Vec::new(), Vec::new()),
        }
    }

    /// One report from a hook or an in-band sequence, applied to the row.
    ///
    /// `event` is the hook's name for Claude Code (`SessionStart`,
    /// `UserPromptSubmit`, `Stop`, `Notification`, `PermissionRequest`),
    /// `codex` for Codex's `notify`, and `osc` for the in-band sequence,
    /// whose payload is `{"status": …, "session": …}`.
    pub fn observe_hook(&self, id: &str, event: &str, payload: &serde_json::Value) {
        // Codex's notify names a thread id that is not its session id — the
        // one `codex resume` wants is in the rollout file it writes, found
        // by the directory it ran in. Looked up before the lock: it reads
        // the filesystem.
        let codex_rollout = (event == "codex")
            .then(|| {
                let cwd = self
                    .sessions
                    .lock()
                    .expect("no lock is held across an await")
                    .get(id)
                    .map(|e| e.place.cwd.clone());
                cwd.and_then(|c| codex_rollout_for(&c))
            })
            .flatten();
        let text = |key: &str| payload.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        let claude = || (text("session_id"), text("transcript_path"));
        let (status, (session, transcript)): (Option<&str>, _) = match event {
            "SessionStart" => (None, claude()),
            "UserPromptSubmit" => (Some("working"), claude()),
            "Stop" => (Some("done"), claude()),
            "PermissionRequest" => (Some("blocked"), claude()),
            "Notification" => (
                match text("notification_type").as_deref() {
                    Some("permission_prompt") => Some("blocked"),
                    Some("idle_prompt") => Some("idle"),
                    _ => None,
                },
                claude(),
            ),
            "codex" => (
                (text("type").as_deref() == Some("agent-turn-complete")).then_some("done"),
                match codex_rollout {
                    Some((session, path)) => (Some(session), Some(path.display().to_string())),
                    None => (None, None),
                },
            ),
            "osc" => (
                match text("status").as_deref() {
                    Some("working") => Some("working"),
                    Some("blocked") => Some("blocked"),
                    Some("idle") => Some("idle"),
                    Some("done") => Some("done"),
                    _ => None,
                },
                (text("session"), None),
            ),
            _ => (None, (None, None)),
        };
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        let Some(entry) = held.get_mut(id) else {
            return;
        };
        if let Some(status) = status {
            entry.agent.status = Some(status.to_owned());
        }
        let mut remember = false;
        if session.is_some() && session != entry.agent.session {
            entry.agent.session = session;
            remember = true;
        }
        let transcript = transcript.map(PathBuf::from);
        if transcript.is_some() && transcript != entry.agent.transcript {
            entry.agent.transcript = transcript;
            remember = true;
        }
        if remember {
            self.record(&held);
        }
    }

    /// What the window itself knows: it just typed a prompt, so the agent
    /// is working until it says otherwise.
    pub(super) fn set_agent_status(&self, id: &str, status: &str) {
        let mut held = self
            .sessions
            .lock()
            .expect("no lock is held across an await");
        if let Some(entry) = held.get_mut(id) {
            entry.agent.status = Some(status.to_owned());
        }
    }

    /// The agent's own transcript, read from the file its hooks named and
    /// folded into the same rows a session's transcript uses — so the pane
    /// that reads one reads the other. Claude Code's JSONL is understood;
    /// a tool that named no file gets an empty transcript with a notice.
    pub fn transcript(&self, id: &str) -> Result<TranscriptView, AppError> {
        let (harness, path) = {
            let held = self
                .sessions
                .lock()
                .expect("no lock is held across an await");
            let entry = held
                .get(id)
                .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?;
            (entry.harness, entry.agent.transcript.clone())
        };
        let Some(path) = path else {
            return Ok(notice(format!(
                "{} has not named a transcript yet; it does once its hooks report.",
                harness.label()
            )));
        };
        let text = std::fs::read_to_string(&path).map_err(|e| {
            AppError::Supervisor(format!("{} could not be read: {e}", path.display()))
        })?;
        let entries = if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("rollout-"))
        {
            fold_codex(&text)
        } else {
            fold_claude(&text)
        };
        Ok(TranscriptView {
            entries,
            model: None,
            cost_usd: 0.0,
            from_record: true,
        })
    }
}

/// The newest Codex rollout written for `cwd`: its session id and path.
/// Rollouts live under `~/.codex/sessions/<y>/<m>/<d>/rollout-<stamp>-<id>.jsonl`
/// and open with a `session_meta` line naming the directory they ran in.
pub(super) fn codex_rollout_for(cwd: &std::path::Path) -> Option<(String, PathBuf)> {
    use std::io::BufRead;
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let root = PathBuf::from(home).join(".codex").join("sessions");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                && let Ok(meta) = entry.metadata()
                && let Ok(modified) = meta.modified()
            {
                files.push((modified, path));
            }
        }
    }
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let wanted = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    for (_, path) in files.into_iter().take(40) {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut first = String::new();
        if std::io::BufReader::new(file).read_line(&mut first).is_err() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&first) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
            continue;
        }
        let same = v
            .pointer("/payload/cwd")
            .and_then(|c| c.as_str())
            .map(PathBuf::from)
            .is_some_and(|c| c.canonicalize().unwrap_or_else(|_| c.clone()) == wanted);
        if !same {
            continue;
        }
        if let Some(id) = v
            .pointer("/payload/id")
            .or_else(|| v.pointer("/payload/session_id"))
            .and_then(|i| i.as_str())
        {
            return Some((id.to_owned(), path));
        }
    }
    None
}

/// Codex's rollout: `response_item` messages by role with `input_text` /
/// `output_text` blocks, `function_call` and `function_call_output` for
/// tools, `event_msg task_complete` for a turn. Developer messages and the
/// instruction blocks Codex injects as the user are skipped.
fn fold_codex(text: &str) -> Vec<TranscriptEntry> {
    let mut out: Vec<TranscriptEntry> = Vec::new();
    let mut n = 0u32;
    let mut next = move || {
        n += 1;
        format!("c{n}")
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = v.get("payload").cloned().unwrap_or(serde_json::Value::Null);
        let ptype = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match (v.get("type").and_then(|t| t.as_str()).unwrap_or(""), ptype) {
            ("response_item", "message") => {
                let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if role != "user" && role != "assistant" {
                    continue;
                }
                let body: String = payload
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|c| {
                        c.iter()
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                let injected = role == "user"
                    && (body.starts_with("# AGENTS.md")
                        || body.starts_with('<')
                        || body.starts_with("## "));
                if body.trim().is_empty() || injected {
                    continue;
                }
                if role == "user" {
                    out.push(TranscriptEntry::User {
                        id: next(),
                        text: body,
                    });
                } else {
                    out.push(TranscriptEntry::Agent {
                        id: next(),
                        text: body,
                        streaming: false,
                    });
                }
            }
            ("response_item", "function_call") => {
                let name = payload
                    .get("name")
                    .and_then(|t| t.as_str())
                    .unwrap_or("tool")
                    .to_owned();
                let args = payload
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .unwrap_or("");
                let subject = serde_json::from_str::<serde_json::Value>(args)
                    .ok()
                    .and_then(|a| {
                        ["cmd", "command", "path", "file_path", "pattern"]
                            .iter()
                            .find_map(|k| {
                                a.get(k).map(|x| {
                                    x.as_str()
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| x.to_string())
                                })
                            })
                    })
                    .unwrap_or_else(|| args.chars().take(120).collect());
                let id = payload
                    .get("call_id")
                    .and_then(|t| t.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(&mut next);
                out.push(TranscriptEntry::Tool {
                    id,
                    name,
                    subject,
                    status: "ok".to_owned(),
                    output: String::new(),
                    truncated: false,
                    preview: None,
                    ms: 0,
                });
            }
            ("response_item", "function_call_output") => {
                let answer = payload
                    .get("call_id")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let text = match payload.get("output") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                if let Some(TranscriptEntry::Tool {
                    output, truncated, ..
                }) = out
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, TranscriptEntry::Tool { id, .. } if id == answer))
                {
                    let mut t: String = text.chars().take(4000).collect();
                    if t.len() < text.len() {
                        *truncated = true;
                        t.push('…');
                    }
                    *output = t;
                }
            }
            ("event_msg", "task_complete") => {
                out.push(TranscriptEntry::Turn {
                    id: next(),
                    outcome: "completed".to_owned(),
                    detail: String::new(),
                    cost_usd: 0.0,
                });
            }
            _ => {}
        }
    }
    out
}

fn notice(message: String) -> TranscriptView {
    TranscriptView {
        entries: vec![TranscriptEntry::Notice {
            id: "notice".to_owned(),
            level: "info".to_owned(),
            message,
        }],
        model: None,
        cost_usd: 0.0,
        from_record: true,
    }
}

/// Claude Code's transcript: one JSON object per line, `type` user or
/// assistant, `message.content` a string or a list of blocks — text,
/// tool_use, tool_result. Tool results are attached to the call they
/// answer by id. Meta lines and anything unknown are skipped.
fn fold_claude(text: &str) -> Vec<TranscriptEntry> {
    let mut out: Vec<TranscriptEntry> = Vec::new();
    let mut n = 0u32;
    let mut next = move || {
        n += 1;
        format!("t{n}")
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("isMeta").and_then(|m| m.as_bool()) == Some(true) {
            continue;
        }
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if kind != "user" && kind != "assistant" {
            continue;
        }
        let Some(message) = v.get("message") else {
            continue;
        };
        let say = |out: &mut Vec<TranscriptEntry>, id: String, s: String| {
            if kind == "user" {
                out.push(TranscriptEntry::User { id, text: s });
            } else {
                out.push(TranscriptEntry::Agent {
                    id,
                    text: s,
                    streaming: false,
                });
            }
        };
        match message.get("content") {
            Some(serde_json::Value::String(s)) => say(&mut out, next(), s.clone()),
            Some(serde_json::Value::Array(blocks)) => {
                for block in blocks {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            let s = block
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_owned();
                            if !s.trim().is_empty() {
                                say(&mut out, next(), s);
                            }
                        }
                        Some("tool_use") => {
                            let name = block
                                .get("name")
                                .and_then(|t| t.as_str())
                                .unwrap_or("tool")
                                .to_owned();
                            let input = block
                                .get("input")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            let subject =
                                ["command", "file_path", "pattern", "description", "prompt"]
                                    .iter()
                                    .find_map(|k| {
                                        input.get(k).and_then(|x| x.as_str()).map(str::to_owned)
                                    })
                                    .unwrap_or_else(|| {
                                        input.to_string().chars().take(120).collect()
                                    });
                            let id = block
                                .get("id")
                                .and_then(|t| t.as_str())
                                .map(str::to_owned)
                                .unwrap_or_else(&mut next);
                            out.push(TranscriptEntry::Tool {
                                id,
                                name,
                                subject,
                                status: "ok".to_owned(),
                                output: String::new(),
                                truncated: false,
                                preview: None,
                                ms: 0,
                            });
                        }
                        Some("tool_result") => {
                            let answer = block
                                .get("tool_use_id")
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let text = match block.get("content") {
                                Some(serde_json::Value::String(s)) => s.clone(),
                                Some(serde_json::Value::Array(parts)) => parts
                                    .iter()
                                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            let failed =
                                block.get("is_error").and_then(|e| e.as_bool()) == Some(true);
                            if let Some(TranscriptEntry::Tool {
                                output,
                                status,
                                truncated,
                                ..
                            }) = out.iter_mut().rev().find(
                                |e| matches!(e, TranscriptEntry::Tool { id, .. } if id == answer),
                            ) {
                                let mut t: String = text.chars().take(4000).collect();
                                if t.len() < text.len() {
                                    *truncated = true;
                                    t.push('…');
                                }
                                *output = t;
                                if failed {
                                    *status = "failed".to_owned();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claude_transcript_folds_into_rows_with_results_on_their_calls() {
        let text = r#"{"type":"user","message":{"role":"user","content":"fix the tests"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"On it."},{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"cargo test"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"ok. 3 passed","is_error":false}]}}
{"type":"user","isMeta":true,"message":{"content":"ignored"}}
not json
{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}"#;
        let rows = fold_claude(text);
        assert_eq!(rows.len(), 4);
        assert!(matches!(&rows[0], TranscriptEntry::User { text, .. } if text == "fix the tests"));
        assert!(
            matches!(&rows[2], TranscriptEntry::Tool { name, subject, output, status, .. }
            if name == "Bash" && subject == "cargo test" && output == "ok. 3 passed" && status == "ok")
        );
        assert!(matches!(&rows[3], TranscriptEntry::Agent { text, .. } if text == "Done."));
    }

    #[test]
    fn a_codex_rollout_folds_and_skips_what_codex_injected() {
        let text = r##"{"type":"session_meta","payload":{"id":"s","cwd":"/x"}}
{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /x"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}
{"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"f1","arguments":"{\"cmd\":\"ls\"}"}}
{"type":"response_item","payload":{"type":"function_call_output","call_id":"f1","output":"a\nb"}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hey!"}]}}
{"type":"event_msg","payload":{"type":"task_complete"}}"##;
        let rows = fold_codex(text);
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert!(matches!(&rows[0], TranscriptEntry::User { text, .. } if text == "hi"));
        assert!(
            matches!(&rows[1], TranscriptEntry::Tool { subject, output, .. } if subject == "ls" && output == "a\nb")
        );
        assert!(matches!(&rows[2], TranscriptEntry::Agent { text, .. } if text == "Hey!"));
        assert!(matches!(&rows[3], TranscriptEntry::Turn { .. }));
    }

    #[test]
    fn hook_events_move_the_status_and_keep_the_session() {
        let hosted = Hosted::default();
        hosted.sessions.lock().unwrap().insert(
            "h1".into(),
            super::super::Held {
                harness: axio_pty::Harness::Claude,
                live: None,
                app: None,
                transport: "pty".into(),
                stopped: false,
                number: 1,
                place: super::super::Place {
                    cwd: ".".into(),
                    repo: ".".into(),
                    branch: None,
                },
                group: None,
                title: None,
                args: String::new(),
                agent: AgentState::default(),
            },
        );
        let payload = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        hosted.observe_hook(
            "h1",
            "UserPromptSubmit",
            &payload(r#"{"session_id":"s1","transcript_path":"/t.jsonl"}"#),
        );
        let v = hosted.list().remove(0);
        assert_eq!(v.agent_status.as_deref(), Some("working"));
        assert_eq!(v.provider_session.as_deref(), Some("s1"));
        hosted.observe_hook(
            "h1",
            "Notification",
            &payload(r#"{"notification_type":"permission_prompt"}"#),
        );
        assert_eq!(
            hosted.list().remove(0).agent_status.as_deref(),
            Some("blocked")
        );
        hosted.observe_hook("h1", "Stop", &payload(r#"{}"#));
        assert_eq!(
            hosted.list().remove(0).agent_status.as_deref(),
            Some("done")
        );
        hosted.observe_hook(
            "h1",
            "codex",
            &payload(r#"{"type":"agent-turn-complete","thread-id":"c9"}"#),
        );
        // Codex's notify ends a turn; its session comes from a rollout on
        // disk, of which this row has none, so the id it had stays.
        let v = hosted.list().remove(0);
        assert_eq!(v.agent_status.as_deref(), Some("done"));
        assert_eq!(v.provider_session.as_deref(), Some("s1"));
        hosted.observe_hook("h1", "osc", &payload(r#"{"status":"working"}"#));
        assert_eq!(
            hosted.list().remove(0).agent_status.as_deref(),
            Some("working")
        );
        assert!(
            hosted.transcript("h1").is_err(),
            "the transcript file does not exist"
        );
    }
}
