//! Codex, driven through its own `app-server` rather than a terminal.
//!
//! `codex app-server` speaks JSON-RPC over stdio, one JSON object per line:
//! requests from here (`initialize`, `thread/start`, `turn/start`),
//! notifications from it (`item/started`, `item/agentMessage/delta`,
//! `item/completed`, `turn/completed`), and requests *from* it that want an
//! answer — `item/commandExecution/requestApproval`, `item/fileChange/
//! requestApproval` — which is how an approval reaches a person as a
//! question instead of a prompt drawn in a terminal nobody is watching.
//!
//! Everything it says is folded into the same transcript rows a session
//! uses, so the pane that reads one reads the other; nothing here draws.
//! The terminal remains the universal way in: this is the better way for
//! the one tool that offers it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, oneshot};

use crate::model::{AppError, TranscriptEntry, TranscriptView};

/// A question the agent is waiting on, as a surface shows it.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct HostedApproval {
    pub id: String,
    /// `command` or `edit`.
    pub kind: String,
    /// The command, or the files.
    pub subject: String,
    pub detail: String,
}

/// What the agent has said so far, and what it is waiting on.
#[derive(Default)]
struct Folded {
    entries: Vec<TranscriptEntry>,
    /// `working`, `blocked`, `idle`, `done`.
    status: String,
    approvals: Vec<(HostedApproval, serde_json::Value)>,
    turn: Option<String>,
    thread: Option<String>,
    model: Option<String>,
}

pub struct CodexSession {
    child: Mutex<Option<tokio::process::Child>>,
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>,
    folded: Arc<Mutex<Folded>>,
    /// Signalled whenever anything changes, like a terminal's `wrote`.
    changed: Arc<Notify>,
    cwd: std::path::PathBuf,
    approval_policy: &'static str,
    sandbox: &'static str,
    model: Option<String>,
    effort: Option<String>,
    /// A thread from an earlier run, to resume instead of starting one.
    resume_from: Option<String>,
}

impl CodexSession {
    /// Start `codex app-server` in `cwd` and shake hands. No thread yet; the
    /// first prompt opens one, or resumes `thread` when it is known.
    pub async fn spawn(
        cwd: &Path,
        permission: Option<&str>,
        model: Option<String>,
        effort: Option<String>,
        extra_args: &[String],
        resume_from: Option<String>,
    ) -> Result<Arc<Self>, AppError> {
        let exe = axio_pty::Harness::Codex
            .locate()
            .ok_or_else(|| AppError::Supervisor("codex is not on this machine".to_owned()))?;
        let mut command = tokio::process::Command::new(exe);
        command
            .args(extra_args)
            .arg("app-server")
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        for name in axio_pty::stripped_names() {
            command.env_remove(name);
        }
        let mut child = command
            .spawn()
            .map_err(|e| AppError::Supervisor(format!("codex app-server could not start: {e}")))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (approval_policy, sandbox) = match permission.unwrap_or("") {
            "ask" => ("untrusted", "read-only"),
            "auto" => ("on-request", "workspace-write"),
            "full" => ("never", "danger-full-access"),
            _ => ("on-request", "workspace-write"),
        };
        let session = Arc::new(Self {
            child: Mutex::new(Some(child)),
            stdin: tokio::sync::Mutex::new(stdin),
            next: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            folded: Arc::new(Mutex::new(Folded {
                status: "idle".to_owned(),
                ..Folded::default()
            })),
            changed: Arc::new(Notify::new()),
            cwd: cwd.to_path_buf(),
            approval_policy,
            sandbox,
            model,
            effort,
            resume_from,
        });
        session.read_forever(stdout);
        session
            .request(
                "initialize",
                serde_json::json!({
                    "clientInfo": {"name": "axio", "title": "axio", "version": env!("CARGO_PKG_VERSION")},
                    "capabilities": {"experimentalApi": true}
                }),
            )
            .await?;
        session
            .notify("initialized", serde_json::Value::Null)
            .await?;
        Ok(session)
    }

    pub fn changed(&self) -> Arc<Notify> {
        Arc::clone(&self.changed)
    }

    pub fn thread(&self) -> Option<String> {
        self.folded
            .lock()
            .expect("no lock across an await")
            .thread
            .clone()
    }

    pub fn status(&self) -> String {
        self.folded
            .lock()
            .expect("no lock across an await")
            .status
            .clone()
    }

    pub fn alive(&self) -> bool {
        let mut child = self.child.lock().expect("no lock across an await");
        match child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    pub fn transcript(&self) -> TranscriptView {
        let f = self.folded.lock().expect("no lock across an await");
        TranscriptView {
            entries: f.entries.clone(),
            model: f.model.clone(),
            cost_usd: 0.0,
            from_record: false,
        }
    }

    pub fn approvals(&self) -> Vec<HostedApproval> {
        self.folded
            .lock()
            .expect("no lock across an await")
            .approvals
            .iter()
            .map(|(a, _)| a.clone())
            .collect()
    }

    /// Open the thread if there is none — resuming the earlier one when
    /// there was one — and start a turn with `prompt`.
    pub async fn prompt(&self, prompt: &str) -> Result<(), AppError> {
        let resume = self.resume_from.as_deref();
        let thread = match self.thread() {
            Some(t) => t,
            None => {
                let mut params = serde_json::json!({
                    "cwd": self.cwd,
                    "approvalPolicy": self.approval_policy,
                    "sandbox": self.sandbox,
                });
                if let Some(model) = &self.model {
                    params["model"] = serde_json::Value::String(model.clone());
                }
                let response = match resume {
                    Some(id) => {
                        params["threadId"] = serde_json::Value::String(id.to_owned());
                        self.request("thread/resume", params).await?
                    }
                    None => self.request("thread/start", params).await?,
                };
                let id = response
                    .pointer("/thread/id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::Supervisor("codex opened no thread".to_owned()))?
                    .to_owned();
                self.folded.lock().expect("no lock across an await").thread = Some(id.clone());
                id
            }
        };
        {
            let mut f = self.folded.lock().expect("no lock across an await");
            let id = format!("u{}", f.entries.len());
            f.entries.push(TranscriptEntry::User {
                id,
                text: prompt.to_owned(),
            });
            f.status = "working".to_owned();
        }
        self.changed.notify_waiters();
        let mut params = serde_json::json!({
            "threadId": thread,
            "input": [{"type": "text", "text": prompt}],
        });
        if let Some(effort) = &self.effort {
            params["effort"] = serde_json::Value::String(effort.clone());
        }
        self.request("turn/start", params).await?;
        Ok(())
    }

    /// Answer a question: `accept`, `decline`, `cancel` or `acceptForSession`.
    pub async fn decide(&self, approval: &str, decision: &str) -> Result<(), AppError> {
        let rpc_id = {
            let mut f = self.folded.lock().expect("no lock across an await");
            let at = f
                .approvals
                .iter()
                .position(|(a, _)| a.id == approval)
                .ok_or_else(|| AppError::NoSuchSession(format!("no question {approval}")))?;
            let (_, id) = f.approvals.remove(at);
            if f.approvals.is_empty() {
                f.status = "working".to_owned();
            }
            id
        };
        self.changed.notify_waiters();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": rpc_id,
            "result": {"decision": decision},
        }))
        .await
    }

    pub async fn interrupt(&self) -> Result<(), AppError> {
        let (thread, turn) = {
            let f = self.folded.lock().expect("no lock across an await");
            (f.thread.clone(), f.turn.clone())
        };
        if let (Some(thread), Some(turn)) = (thread, turn) {
            self.request(
                "turn/interrupt",
                serde_json::json!({"threadId": thread, "turnId": turn}),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn close(&self) {
        let child = self.child.lock().expect("no lock across an await").take();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let mut f = self.folded.lock().expect("no lock across an await");
        if f.status != "done" {
            f.status = "idle".to_owned();
        }
        self.changed.notify_waiters();
    }

    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("no lock across an await")
            .insert(id, tx);
        let mut message = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message).await?;
        match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(e))) => Err(AppError::Supervisor(format!("codex {method}: {e}"))),
            Ok(Err(_)) => Err(AppError::Supervisor(format!(
                "codex went away during {method}"
            ))),
            Err(_) => Err(AppError::Supervisor(format!(
                "codex did not answer {method}"
            ))),
        }
    }

    async fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), AppError> {
        let mut message = serde_json::json!({"jsonrpc": "2.0", "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message).await
    }

    async fn send(&self, message: serde_json::Value) -> Result<(), AppError> {
        let mut line = message.to_string();
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AppError::Supervisor(format!("codex stdin: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| AppError::Supervisor(format!("codex stdin: {e}")))
    }

    fn read_forever(self: &Arc<Self>, stdout: tokio::process::ChildStdout) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                me.receive(message);
            }
            // The process is gone. Whoever is waiting learns it now.
            me.pending.lock().expect("no lock across an await").clear();
            let mut f = me.folded.lock().expect("no lock across an await");
            if f.status == "working" || f.status == "blocked" {
                f.status = "idle".to_owned();
            }
            drop(f);
            me.changed.notify_waiters();
        });
    }

    fn receive(self: &Arc<Self>, message: serde_json::Value) {
        let method = message
            .get("method")
            .and_then(|m| m.as_str())
            .map(str::to_owned);
        let id = message.get("id").cloned();
        match (method, id) {
            // A response to something asked.
            (None, Some(id)) => {
                let Some(n) = id.as_u64() else { return };
                if let Some(tx) = self
                    .pending
                    .lock()
                    .expect("no lock across an await")
                    .remove(&n)
                {
                    let outcome = match message.get("error") {
                        Some(e) => Err(e
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("error")
                            .to_owned()),
                        None => Ok(message
                            .get("result")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null)),
                    };
                    let _ = tx.send(outcome);
                }
            }
            // A question from it.
            (Some(method), Some(id)) => {
                let params = message
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                self.ask(&method, id, &params);
            }
            // News.
            (Some(method), None) => {
                let params = message
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                self.fold(&method, &params);
            }
            (None, None) => {}
        }
    }

    fn ask(self: &Arc<Self>, method: &str, rpc_id: serde_json::Value, params: &serde_json::Value) {
        let text = |k: &str| {
            params
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned()
        };
        let approval = match method {
            "item/commandExecution/requestApproval" => HostedApproval {
                id: format!("a{}", rpc_id),
                kind: "command".to_owned(),
                subject: text("command"),
                detail: text("reason"),
            },
            "item/fileChange/requestApproval" => HostedApproval {
                id: format!("a{}", rpc_id),
                kind: "edit".to_owned(),
                subject: params
                    .get("changes")
                    .and_then(|c| c.as_array())
                    .map(|c| {
                        c.iter()
                            .filter_map(|x| x.get("path").and_then(|p| p.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
                detail: text("reason"),
            },
            _ => {
                // Something this does not know how to answer: decline, so
                // the agent is not left waiting on a question nobody sees.
                let me = Arc::clone(self);
                tokio::spawn(async move {
                    let _ = me
                        .send(serde_json::json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"decision": "decline"}}))
                        .await;
                });
                return;
            }
        };
        let mut f = self.folded.lock().expect("no lock across an await");
        f.approvals.push((approval, rpc_id));
        f.status = "blocked".to_owned();
        drop(f);
        self.changed.notify_waiters();
    }

    fn fold(&self, method: &str, params: &serde_json::Value) {
        let mut f = self.folded.lock().expect("no lock across an await");
        match method {
            "turn/started" => {
                f.turn = params
                    .pointer("/turn/id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                f.status = "working".to_owned();
            }
            "turn/completed" => {
                let status = params
                    .pointer("/turn/status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("completed")
                    .to_owned();
                let n = f.entries.len();
                f.entries.push(TranscriptEntry::Turn {
                    id: format!("turn{n}"),
                    outcome: status,
                    detail: String::new(),
                    cost_usd: 0.0,
                });
                f.turn = None;
                f.status = "done".to_owned();
            }
            "item/started" | "item/completed" => {
                if let Some(item) = params.get("item") {
                    upsert(&mut f.entries, item, method == "item/completed");
                }
            }
            "item/agentMessage/delta" => {
                let id = params.get("itemId").and_then(|v| v.as_str()).unwrap_or("");
                let delta = params.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(TranscriptEntry::Agent { text, .. }) = f
                    .entries
                    .iter_mut()
                    .find(|e| matches!(e, TranscriptEntry::Agent { id: i, .. } if i == id))
                {
                    text.push_str(delta);
                } else {
                    f.entries.push(TranscriptEntry::Agent {
                        id: id.to_owned(),
                        text: delta.to_owned(),
                        streaming: true,
                    });
                }
            }
            "item/commandExecution/outputDelta" => {
                let id = params.get("itemId").and_then(|v| v.as_str()).unwrap_or("");
                let delta = params.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(TranscriptEntry::Tool { output, .. }) = f
                    .entries
                    .iter_mut()
                    .find(|e| matches!(e, TranscriptEntry::Tool { id: i, .. } if i == id))
                    && output.len() < 16_000
                {
                    output.push_str(delta);
                }
            }
            "thread/started" => {
                if let Some(id) = params.pointer("/thread/id").and_then(|v| v.as_str()) {
                    f.thread = Some(id.to_owned());
                }
                if let Some(model) = params.pointer("/thread/model").and_then(|v| v.as_str()) {
                    f.model = Some(model.to_owned());
                }
            }
            _ => return,
        }
        drop(f);
        self.changed.notify_waiters();
    }
}

/// An item as it started or completed, into the rows: one row per item id.
fn upsert(entries: &mut Vec<TranscriptEntry>, item: &serde_json::Value, completed: bool) {
    let id = item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let text = |k: &str| {
        item.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned()
    };
    let row = match item.get("type").and_then(|t| t.as_str()) {
        Some("agentMessage") => TranscriptEntry::Agent {
            id: id.clone(),
            text: text("text"),
            streaming: !completed,
        },
        Some("reasoning") => TranscriptEntry::Reasoning {
            id: id.clone(),
            text: item
                .get("summary")
                .and_then(|s| s.as_array())
                .map(|s| {
                    s.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        },
        Some("commandExecution") => TranscriptEntry::Tool {
            id: id.clone(),
            name: "shell".to_owned(),
            subject: text("command"),
            status: match (completed, item.get("exitCode").and_then(|e| e.as_i64())) {
                (false, _) => "running".to_owned(),
                (true, Some(0)) | (true, None) => "ok".to_owned(),
                (true, Some(_)) => "failed".to_owned(),
            },
            output: text("aggregatedOutput"),
            truncated: false,
            preview: None,
            ms: item.get("durationMs").and_then(|d| d.as_u64()).unwrap_or(0),
        },
        Some("fileChange") => TranscriptEntry::Tool {
            id: id.clone(),
            name: "edit".to_owned(),
            subject: item
                .get("changes")
                .and_then(|c| c.as_array())
                .map(|c| {
                    c.iter()
                        .filter_map(|x| x.get("path").and_then(|p| p.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            status: if completed {
                "ok".to_owned()
            } else {
                "running".to_owned()
            },
            output: String::new(),
            truncated: false,
            preview: None,
            ms: 0,
        },
        _ => return,
    };
    let key = |e: &TranscriptEntry| match e {
        TranscriptEntry::Agent { id, .. }
        | TranscriptEntry::Reasoning { id, .. }
        | TranscriptEntry::Tool { id, .. } => id.clone(),
        _ => String::new(),
    };
    match entries.iter().position(|e| key(e) == id) {
        Some(at) => {
            // A completed agent message with an empty text keeps what the
            // deltas built; the completion carries the final text otherwise.
            if let (
                TranscriptEntry::Agent { text: new, .. },
                TranscriptEntry::Agent { text: old, .. },
            ) = (&row, &entries[at])
                && new.is_empty()
            {
                let kept = old.clone();
                entries[at] = row;
                if let TranscriptEntry::Agent { text, .. } = &mut entries[at] {
                    *text = kept;
                }
            } else if let (
                TranscriptEntry::Tool { output: new, .. },
                TranscriptEntry::Tool { output: old, .. },
            ) = (&row, &entries[at])
                && new.is_empty()
            {
                let kept = old.clone();
                entries[at] = row;
                if let TranscriptEntry::Tool { output, .. } = &mut entries[at] {
                    *output = kept;
                }
            } else {
                entries[at] = row;
            }
        }
        None => entries.push(row),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The protocol, folded, without a process: what the surface would show.
    #[tokio::test]
    async fn notifications_fold_into_rows_and_questions_block() {
        let folded = Arc::new(Mutex::new(Folded::default()));
        let session = Arc::new(fake(folded));
        let j = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        session.fold("turn/started", &j(r#"{"turn":{"id":"t1"}}"#));
        session.fold(
            "item/started",
            &j(r#"{"item":{"id":"m1","type":"agentMessage","text":""}}"#),
        );
        session.fold(
            "item/agentMessage/delta",
            &j(r#"{"itemId":"m1","delta":"Hel"}"#),
        );
        session.fold(
            "item/agentMessage/delta",
            &j(r#"{"itemId":"m1","delta":"lo"}"#),
        );
        session.fold("item/started", &j(r#"{"item":{"id":"c1","type":"commandExecution","command":"ls","status":"inProgress"}}"#));
        session.ask(
            "item/commandExecution/requestApproval",
            j("7"),
            &j(r#"{"itemId":"c1","command":"ls"}"#),
        );
        assert_eq!(session.status(), "blocked");
        assert_eq!(session.approvals()[0].subject, "ls");
        session.fold("item/completed", &j(r#"{"item":{"id":"c1","type":"commandExecution","command":"ls","exitCode":0,"aggregatedOutput":"a\nb"}}"#));
        session.fold(
            "item/completed",
            &j(r#"{"item":{"id":"m1","type":"agentMessage","text":""}}"#),
        );
        session.fold(
            "turn/completed",
            &j(r#"{"turn":{"id":"t1","status":"completed"}}"#),
        );
        let view = session.transcript();
        assert!(
            matches!(&view.entries[0], TranscriptEntry::Agent { text, streaming, .. } if text == "Hello" && !streaming)
        );
        assert!(
            matches!(&view.entries[1], TranscriptEntry::Tool { subject, output, status, .. } if subject == "ls" && output == "a\nb" && status == "ok")
        );
        assert!(
            matches!(&view.entries[2], TranscriptEntry::Turn { outcome, .. } if outcome == "completed")
        );
        assert_eq!(session.status(), "done");
    }

    fn fake(folded: Arc<Mutex<Folded>>) -> CodexSession {
        // No process behind it: only `fold`, `ask` and the readers are used.
        let (_tx, rx) = tokio::io::duplex(8);
        let _ = rx;
        let child = tokio::process::Command::new(if cfg!(windows) { "cmd.exe" } else { "sh" })
            .args(if cfg!(windows) {
                vec!["/c", "exit"]
            } else {
                vec!["-c", "exit"]
            })
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = child;
        let stdin = child.stdin.take().unwrap();
        CodexSession {
            child: Mutex::new(Some(child)),
            stdin: tokio::sync::Mutex::new(stdin),
            next: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            folded,
            changed: Arc::new(Notify::new()),
            cwd: std::env::temp_dir(),
            approval_policy: "on-request",
            sandbox: "workspace-write",
            model: None,
            effort: None,
            resume_from: None,
        }
    }

    /// Against the real binary, when it is here: the handshake and a thread,
    /// no turn — no tokens spent.
    #[tokio::test]
    async fn the_real_app_server_shakes_hands_and_opens_a_thread() {
        if axio_pty::Harness::Codex.locate().is_none()
            || std::env::var_os("AXIO_LIVE_CODEX").is_none()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let session = CodexSession::spawn(dir.path(), Some("ask"), None, None, &[], None)
            .await
            .unwrap();
        let response = session
            .request("thread/start", serde_json::json!({"cwd": dir.path(), "approvalPolicy": "untrusted", "sandbox": "read-only"}))
            .await
            .unwrap();
        assert!(response.pointer("/thread/id").is_some(), "{response}");
        session.close().await;
    }
}
