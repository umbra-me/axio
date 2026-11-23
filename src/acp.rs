//! `axio acp`: the Agent Client Protocol (version 1) on stdin and stdout, so
//! an editor or Axio Atrium can run axio as its agent. One JSON-RPC message
//! per line. Prompts run one at a time on the main thread; the reader thread
//! hands the client's replies to permission requests straight to the waiting
//! turn and turns `session/cancel` into a stop.

use crate::agent::{Agent, Ui};
use crate::message::{Block, Message};
use crate::provider::{Delta, Model, Stop, Usage};
use crate::skills::Library;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const PROTOCOL_VERSION: u64 = 1;
/// Characters of a tool's output shown to the client; the model gets it all.
const OUTPUT_LIMIT: usize = 20_000;
/// Files larger than this are shown as the edit alone, not the whole file.
const DIFF_FILE_LIMIT: u64 = 1024 * 1024;
const INVALID_PARAMS: i64 = -32602;
const NOT_FOUND: i64 = -32601;
const INTERNAL: i64 = -32603;

/// Builds a session's agent for a folder: the session id to continue, if
/// any, and the client's MCP servers in `mcp.json` form.
pub type Make = Box<dyn FnMut(&Path, Option<&str>, &Value) -> Result<(Agent, Library), String>>;

/// The connection: our output, requests waiting for the client's reply, and
/// the running prompt's stop flag.
pub struct Peer {
    out: Mutex<Box<dyn Write + Send>>,
    waiting: Mutex<HashMap<u64, Sender<Value>>>,
    next_id: AtomicU64,
    next_call: AtomicU64,
    running: Mutex<Option<String>>,
    cancel: AtomicBool,
    closed: AtomicBool,
}

impl Peer {
    pub fn new(out: Box<dyn Write + Send>) -> Arc<Peer> {
        Arc::new(Peer {
            out: Mutex::new(out),
            waiting: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            next_call: AtomicU64::new(0),
            running: Mutex::new(None),
            cancel: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    fn send(&self, v: &Value) {
        if let Ok(mut o) = self.out.lock()
            && writeln!(o, "{v}").and_then(|_| o.flush()).is_err()
        {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    fn reply(&self, id: &Value, r: Result<Value, (i64, String)>) {
        self.send(&match r {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        });
    }

    fn update(&self, session: &str, update: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": "session/update",
                          "params": {"sessionId": session, "update": update}}));
    }

    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.closed.load(Ordering::Relaxed)
    }

    /// Asks the client and waits for its reply. `None` once the prompt is
    /// cancelled or the client has gone.
    fn request(&self, method: &str, params: Value) -> Option<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = channel();
        self.waiting.lock().ok()?.insert(id, tx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(v) => return Some(v),
                Err(RecvTimeoutError::Timeout) if !self.stopped() => continue,
                Err(_) => {
                    if let Ok(mut w) = self.waiting.lock() {
                        w.remove(&id);
                    }
                    return None;
                }
            }
        }
    }

    /// Sorts one message from the client. Replies go to whoever waits and
    /// `session/cancel` stops the running prompt; the rest is returned, to
    /// be handled in order.
    pub fn route(&self, msg: Value) -> Option<Value> {
        if msg.get("method").is_none() {
            let waiter = msg["id"]
                .as_u64()
                .and_then(|id| self.waiting.lock().ok()?.remove(&id));
            if let Some(tx) = waiter {
                let _ = tx.send(msg);
            }
            return None;
        }
        if msg["method"] == "session/cancel" {
            let running = self.running.lock().ok().and_then(|r| r.clone());
            if running.is_some_and(|r| msg["params"]["sessionId"] == r.as_str()) {
                self.cancel.store(true, Ordering::Relaxed);
            }
            return None;
        }
        Some(msg)
    }

    /// The client closed its side: stop whatever runs.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

struct Session {
    agent: Agent,
    library: Library,
    /// Tokens the session's turns have used so far.
    spent: Usage,
}

pub struct Server {
    peer: Arc<Peer>,
    make: Make,
    /// `~/.axio/agent`, for listing saved sessions.
    home: PathBuf,
    sessions: HashMap<String, Session>,
    /// Every model the signed-in providers offer, once listed.
    models: Vec<String>,
    /// Whether the client reads and writes files for axio (its buffers).
    files: Editor,
}

/// The client's file capabilities.
#[derive(Clone, Copy, Default)]
struct Editor {
    read: bool,
    write: bool,
}

impl Server {
    pub fn new(peer: Arc<Peer>, make: Make, home: PathBuf) -> Server {
        Server {
            peer,
            make,
            home,
            sessions: HashMap::new(),
            models: Vec::new(),
            files: Editor::default(),
        }
    }

    /// Handles messages until the client goes, then ends every session.
    pub fn run(&mut self, incoming: Receiver<Value>) {
        loop {
            match incoming.recv_timeout(Duration::from_millis(100)) {
                Ok(msg) => self.dispatch(msg),
                Err(RecvTimeoutError::Timeout) if !self.peer.closed.load(Ordering::Relaxed) => {}
                Err(_) => break,
            }
        }
        for s in self.sessions.values() {
            s.agent.end("other", &mut Quiet);
        }
    }

    fn dispatch(&mut self, msg: Value) {
        let id = msg.get("id").cloned();
        let method = msg["method"].as_str().unwrap_or("").to_string();
        let params = &msg["params"];
        let r = match method.as_str() {
            "initialize" => {
                let fs = &params["clientCapabilities"]["fs"];
                self.files = Editor {
                    read: fs["readTextFile"] == true,
                    write: fs["writeTextFile"] == true,
                };
                Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": {"image": true, "audio": false, "embeddedContext": true},
                    "mcpCapabilities": {"http": true, "sse": true},
                    "sessionCapabilities": {"list": {}, "fork": {}},
                },
                "authMethods": [],
                "agentInfo": {"name": "axio", "title": "Axio", "version": env!("CARGO_PKG_VERSION")},
                }))
            }
            "session/list" => Ok(list(&self.home, params)),
            "authenticate" => Ok(json!({})),
            "session/new" | "session/load" => {
                return self.open(id, &method, params);
            }
            // A copy of a session's history in another folder, to ask about
            // it on the side; the parent goes on untouched.
            "session/fork" => {
                let parent = params["sessionId"].as_str().filter(|s| valid_id(s));
                let found = parent.and_then(|s| crate::session::find(&self.home, s));
                let cwd = params["cwd"]
                    .as_str()
                    .map(PathBuf::from)
                    .filter(|c| c.is_absolute());
                let (Some(found), Some(cwd)) = (found, cwd) else {
                    if let Some(id) = &id {
                        let e = "an axio session and an absolute cwd are required".to_string();
                        self.peer.reply(id, Err((INVALID_PARAMS, e)));
                    }
                    return;
                };
                let mut opened = params.clone();
                match crate::session::fork(&self.home, &found, &cwd) {
                    Ok(path) => opened["sessionId"] = json!(session_id(&path)),
                    Err(e) => {
                        if let Some(id) = &id {
                            self.peer.reply(id, Err((INTERNAL, e)));
                        }
                        return;
                    }
                }
                return self.open(id, &method, &opened);
            }
            "session/prompt" => return self.prompt(id, params),
            "session/set_mode" => self.with(params, |s, _| {
                let mode = params["modeId"].as_str().unwrap_or("");
                set_mode(&s.agent, mode).map(|_| json!({}))
            }),
            "session/set_model" => self.with(params, |s, _| {
                set_option(&mut s.agent, "model", &params["modelId"]).map(|_| json!({}))
            }),
            "session/set_config_option" => {
                let models = self.models.clone();
                self.with(params, |s, _| {
                    let key = params["configId"].as_str().unwrap_or("");
                    set_option(&mut s.agent, key, &params["value"])?;
                    Ok(json!({"configOptions": options(&s.agent, &models)}))
                })
            }
            // Ours: the model list, read in the background at start.
            "_axio/models" => {
                self.models = serde_json::from_value(params["models"].clone()).unwrap_or_default();
                for (sid, s) in &self.sessions {
                    self.peer.update(
                        sid,
                        json!({"sessionUpdate": "config_option_update",
                               "configOptions": options(&s.agent, &self.models)}),
                    );
                }
                return;
            }
            _ => Err((NOT_FOUND, format!("axio has no method {method}"))),
        };
        if let Some(id) = id {
            self.peer.reply(&id, r);
        }
    }

    /// Runs `f` on the session `params` names.
    fn with(
        &mut self,
        params: &Value,
        f: impl FnOnce(&mut Session, &Arc<Peer>) -> Result<Value, (i64, String)>,
    ) -> Result<Value, (i64, String)> {
        let sid = params["sessionId"].as_str().unwrap_or("");
        match self.sessions.get_mut(sid) {
            Some(s) => f(s, &self.peer),
            None => Err((INVALID_PARAMS, format!("no session {sid}"))),
        }
    }

    /// `session/new` and `session/load`.
    fn open(&mut self, id: Option<Value>, method: &str, params: &Value) {
        let reply = |r| {
            if let Some(id) = &id {
                self.peer.reply(id, r);
            }
        };
        let Some(cwd) = params["cwd"].as_str().map(PathBuf::from) else {
            return reply(Err((INVALID_PARAMS, "cwd is required".into())));
        };
        if !cwd.is_absolute() {
            return reply(Err((INVALID_PARAMS, "cwd must be an absolute path".into())));
        }
        let resume = if method == "session/load" || method == "session/fork" {
            match params["sessionId"].as_str() {
                Some(s) if valid_id(s) => Some(s),
                _ => return reply(Err((INVALID_PARAMS, "not an axio session id".into()))),
            }
        } else {
            None
        };
        let servers = mcp_doc(&params["mcpServers"]);
        let (mut agent, library) = match (self.make)(&cwd, resume, &servers) {
            Ok(x) => x,
            Err(e) => return reply(Err((INTERNAL, e))),
        };
        let sid = match (resume, &agent.session) {
            (Some(s), _) => s.to_string(),
            (None, Some(s)) => session_id(&s.path),
            (None, None) => format!("unsaved-{}", self.sessions.len() + 1),
        };
        if let Err(e) = agent.start(
            if resume.is_some() {
                "resume"
            } else {
                "startup"
            },
            &mut Quiet,
        ) {
            return reply(Err((INTERNAL, e)));
        }
        if method == "session/load" {
            replay(&self.peer, &sid, &agent.messages, &agent.cwd);
        }
        let mut result = json!({
            "modes": modes(&agent),
            "configOptions": options(&agent, &self.models),
        });
        if method != "session/load" {
            result["sessionId"] = json!(sid);
        }
        reply(Ok(result));
        self.peer.update(
            &sid,
            json!({"sessionUpdate": "available_commands_update",
                   "availableCommands": commands(&library)}),
        );
        let spent = Usage::default();
        self.sessions.insert(
            sid,
            Session {
                agent,
                library,
                spent,
            },
        );
    }

    fn prompt(&mut self, id: Option<Value>, params: &Value) {
        let peer = self.peer.clone();
        let reply = |r| {
            if let Some(id) = &id {
                peer.reply(id, r);
            }
        };
        let sid = params["sessionId"].as_str().unwrap_or("").to_string();
        let Some(s) = self.sessions.get_mut(&sid) else {
            return reply(Err((INVALID_PARAMS, format!("no session {sid}"))));
        };
        let (text, attached) = content(&params["prompt"]);
        let mode = s.agent.permissions().mode;
        if let Ok(mut r) = peer.running.lock() {
            *r = Some(sid.clone());
        }
        peer.cancel.store(false, Ordering::Relaxed);
        let mut turn = Turn {
            peer: &peer,
            session: sid.clone(),
            cwd: s.agent.cwd.clone(),
            pending: None,
            current: None,
            files: self.files,
            stop: None,
            spoke: false,
            gap: false,
            window: s.agent.model.context_window(),
            spent: Usage::default(),
            model: s.agent.model.clone(),
            fast: s.agent.fast && !s.agent.fast_refused.load(Ordering::Relaxed),
            cost: s.agent.spent(),
            agents: HashMap::new(),
        };
        let result = if text.trim() == "/compact" {
            s.agent.compact("manual", &mut turn).map(|_| ())
        } else {
            match crate::command(&s.library, &text) {
                Some(Err(e)) => Err(e),
                Some(Ok(expanded)) => s.agent.prompt_with(&expanded, attached, &mut turn),
                None => s.agent.prompt_with(&text, attached, &mut turn),
            }
        };
        let stop = turn.stop.take();
        let u = &mut s.spent;
        u.input += turn.spent.input;
        u.output += turn.spent.output;
        u.cache_read += turn.spent.cache_read;
        u.cache_write += turn.spent.cache_write;
        let usage = json!({
            "totalTokens": u.context(),
            "inputTokens": u.input + u.cache_read + u.cache_write,
            "outputTokens": u.output,
            "cachedReadTokens": u.cache_read,
            "cachedWriteTokens": u.cache_write,
        });
        if let Ok(mut r) = peer.running.lock() {
            *r = None;
        }
        let cancelled = peer.cancel.swap(false, Ordering::Relaxed);
        let now = s.agent.permissions().mode;
        if now != mode {
            peer.update(
                &sid,
                json!({"sessionUpdate": "current_mode_update", "currentModeId": now.name()}),
            );
        }
        reply(match result {
            Err(e) if e == crate::agent::ABORTED || cancelled => {
                Ok(json!({"stopReason": "cancelled", "usage": usage}))
            }
            Err(e) => Err((INTERNAL, e)),
            Ok(()) => Ok(json!({"stopReason": match stop {
                Some(Stop::MaxTokens) => "max_tokens",
                Some(Stop::Refusal) => "refusal",
                _ => "end_turn",
            }, "usage": usage})),
        });
    }
}

/// A file name a session id may be: no separators, nothing hidden.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn session_id(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The client's MCP servers as an `mcp.json` document.
pub fn mcp_doc(list: &Value) -> Value {
    let pairs = |v: &Value, key: &str| -> serde_json::Map<String, Value> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p["name"].as_str()?.to_string(), p[key].clone())))
            .collect()
    };
    let mut servers = serde_json::Map::new();
    for s in list.as_array().into_iter().flatten() {
        let Some(name) = s["name"].as_str() else {
            continue;
        };
        let entry = match s["type"].as_str() {
            Some(kind @ ("http" | "sse")) => json!({
                "type": kind,
                "url": s["url"],
                "headers": pairs(&s["headers"], "value"),
            }),
            _ => json!({
                "command": s["command"],
                "args": s["args"].as_array().cloned().unwrap_or_default(),
                "env": pairs(&s["env"], "value"),
            }),
        };
        servers.insert(name.to_string(), entry);
    }
    json!({ "mcpServers": servers })
}

/// The prompt's text, with linked files as `@` mentions, and the blocks
/// that stand on their own (images, embedded file contents).
pub fn content(blocks: &Value) -> (String, Vec<Block>) {
    let mut text = String::new();
    let mut attached = Vec::new();
    for b in blocks.as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => text.push_str(b["text"].as_str().unwrap_or("")),
            Some("image") => {
                if let (Some(m), Some(d)) = (b["mimeType"].as_str(), b["data"].as_str()) {
                    attached.push(Block::Image {
                        media_type: m.into(),
                        data: d.into(),
                    });
                }
            }
            Some("resource_link") => {
                let uri = b["uri"].as_str().unwrap_or("");
                match uri.strip_prefix("file://") {
                    Some(path) if !path.contains('"') => {
                        text.push_str(&format!(" @\"{}\" ", unescape(path)))
                    }
                    _ => text.push_str(&format!(" {uri} ")),
                }
            }
            Some("resource") => {
                let r = &b["resource"];
                let uri = r["uri"].as_str().unwrap_or("");
                if let Some(t) = r["text"].as_str() {
                    attached.push(Block::Text {
                        text: format!("<file uri=\"{uri}\">\n{t}\n</file>"),
                    });
                } else if let (Some(m), Some(d)) = (r["mimeType"].as_str(), r["blob"].as_str())
                    && m.starts_with("image/")
                {
                    attached.push(Block::Image {
                        media_type: m.into(),
                        data: d.into(),
                    });
                }
            }
            _ => {}
        }
    }
    (text, attached)
}

/// `%20` and friends in a file URI's path.
fn unescape(path: &str) -> String {
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const MODES: [(&str, &str, &str); 4] = [
    (
        "default",
        "Ask",
        "Ask before changing files or running commands",
    ),
    (
        "acceptEdits",
        "Accept edits",
        "Change files without asking; ask for commands",
    ),
    (
        "plan",
        "Plan",
        "Read and research only, then propose a plan",
    ),
    ("bypassPermissions", "Bypass permissions", "Never ask"),
];

fn modes(a: &Agent) -> Value {
    json!({
        "currentModeId": a.permissions().mode.name(),
        "availableModes": MODES.iter().map(|(id, name, about)|
            json!({"id": id, "name": name, "description": about})).collect::<Vec<_>>(),
    })
}

fn set_mode(a: &Agent, id: &str) -> Result<(), (i64, String)> {
    let mode = crate::permissions::Mode::parse(id)
        .ok_or_else(|| (INVALID_PARAMS, format!("unknown mode {id}")))?;
    a.set_mode(mode);
    Ok(())
}

/// Model, effort and mode as select options.
fn options(a: &Agent, models: &[String]) -> Value {
    let select =
        |id: &str, name: &str, category: &str, current: &str, values: Vec<(String, String)>| {
            json!({"id": id, "name": name, "category": category, "type": "select",
               "currentValue": current,
               "options": values.into_iter().map(|(v, n)| json!({"value": v, "name": n}))
                   .collect::<Vec<_>>()})
        };
    let current = a.model.to_string();
    let mut names = vec![current.clone()];
    names.extend(models.iter().filter(|m| **m != current).cloned());
    let mut out = vec![select(
        "model",
        "Model",
        "model",
        &current,
        names.into_iter().map(|m| (m.clone(), m)).collect(),
    )];
    let efforts = a.model.efforts();
    if let Some(first) = efforts.first() {
        let effort = a
            .effort
            .clone()
            .filter(|e| efforts.contains(e))
            .or_else(|| a.model.default_effort())
            .unwrap_or_else(|| first.clone());
        out.push(select(
            "effort",
            "Effort",
            "thought_level",
            &effort,
            efforts.iter().map(|e| (e.clone(), e.clone())).collect(),
        ));
    }
    out.push(select(
        "mode",
        "Mode",
        "mode",
        a.permissions().mode.name(),
        MODES
            .iter()
            .map(|(id, name, _)| (id.to_string(), name.to_string()))
            .collect(),
    ));
    json!(out)
}

fn set_option(a: &mut Agent, key: &str, value: &Value) -> Result<(), (i64, String)> {
    let Some(v) = value.as_str().filter(|v| !v.is_empty()) else {
        return Err((INVALID_PARAMS, format!("{key} needs a string value")));
    };
    match key {
        "model" => {
            a.model = Model::parse(v);
            if a.effort
                .as_ref()
                .is_some_and(|e| !a.model.efforts().contains(e))
            {
                a.effort = None;
            }
            Ok(())
        }
        "effort" if a.model.efforts().iter().any(|e| e == v) => {
            a.effort = Some(v.to_string());
            Ok(())
        }
        "effort" => Err((INVALID_PARAMS, format!("{} has no effort {v}", a.model))),
        "mode" => set_mode(a, v),
        _ => Err((INVALID_PARAMS, format!("unknown option {key}"))),
    }
}

/// Commands and invocable skills, and the built-in /compact.
fn commands(lib: &Library) -> Value {
    let mut out = vec![json!({"name": "compact",
        "description": "Summarise the conversation to free context"})];
    for c in &lib.commands {
        out.push(json!({"name": c.name, "description": c.description,
                        "input": {"hint": "arguments"}}));
    }
    for k in lib.skills.iter().filter(|k| k.invocable) {
        out.push(json!({"name": k.name, "description": k.description}));
    }
    json!(out)
}

fn kind(tool: &str) -> &'static str {
    match tool {
        "Read" => "read",
        "Write" | "Edit" => "edit",
        "Bash" => "execute",
        "Grep" | "Glob" => "search",
        "WebFetch" | "WebSearch" => "fetch",
        "TodoWrite" => "think",
        _ => "other",
    }
}

fn title(tool: &str, input: &Value, cwd: &Path) -> String {
    let (_, detail) = crate::ui::tool_title(tool, input, cwd);
    if detail.is_empty() {
        tool.to_string()
    } else {
        format!("{tool} {detail}")
    }
}

fn absolute(cwd: &Path, p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// What the client shows for a call before it runs: where it works and,
/// for Write and Edit, the change as a diff of the whole file.
fn preview(
    tool: &str,
    input: &Value,
    cwd: &Path,
    read: &mut dyn FnMut(&Path) -> Option<String>,
) -> (Value, Value) {
    let Some(path) = input["file_path"].as_str().map(|p| absolute(cwd, p)) else {
        return (json!([]), json!([]));
    };
    let shown = path.display().to_string();
    let locations = json!([{"path": shown}]);
    let old = match tool {
        "Write" | "Edit" => read(&path),
        _ => None,
    };
    let s = |k: &str| input[k].as_str().unwrap_or("");
    let diff = match tool {
        "Write" => json!({"type": "diff", "path": shown, "oldText": old, "newText": s("content")}),
        "Edit" => {
            match old.filter(|o| o.contains(s("old_string")) && !s("old_string").is_empty()) {
                Some(o) => {
                    let new = if input["replace_all"] == true {
                        o.replace(s("old_string"), s("new_string"))
                    } else {
                        o.replacen(s("old_string"), s("new_string"), 1)
                    };
                    json!({"type": "diff", "path": shown, "oldText": o, "newText": new})
                }
                None => json!({"type": "diff", "path": shown,
                           "oldText": s("old_string"), "newText": s("new_string")}),
            }
        }
        _ => return (locations, json!([])),
    };
    (locations, json!([diff]))
}

/// A file's text for a diff, unless it is too large to show whole.
fn disk_text(path: &Path) -> Option<String> {
    let small = std::fs::metadata(path).is_ok_and(|m| m.len() <= DIFF_FILE_LIMIT);
    small.then(|| std::fs::read_to_string(path).ok()).flatten()
}

fn clip(text: &str) -> String {
    if text.chars().count() <= OUTPUT_LIMIT {
        return text.to_string();
    }
    let kept: String = text.chars().take(OUTPUT_LIMIT).collect();
    format!("{kept}\n… (cut; the model saw all of it)")
}

fn text_content(text: &str) -> Value {
    json!([{"type": "content", "content": {"type": "text", "text": clip(text)}}])
}

/// Replays a loaded session as updates, so the client can show it.
fn replay(peer: &Peer, sid: &str, messages: &[Message], cwd: &Path) {
    for m in messages {
        match m {
            Message::User { content } => {
                for b in content {
                    let update = match b {
                        Block::Text { text } if !text.starts_with("[axio]") => json!({
                            "sessionUpdate": "user_message_chunk",
                            "content": {"type": "text", "text": text}}),
                        Block::Image { media_type, data } => json!({
                            "sessionUpdate": "user_message_chunk",
                            "content": {"type": "image", "mimeType": media_type, "data": data}}),
                        Block::ToolResult {
                            id,
                            output,
                            is_error,
                        } => json!({
                            "sessionUpdate": "tool_call_update", "toolCallId": id,
                            "status": if *is_error { "failed" } else { "completed" },
                            "content": text_content(output)}),
                        _ => continue,
                    };
                    peer.update(sid, update);
                }
            }
            Message::Assistant { content } => {
                for b in content {
                    let update = match b {
                        Block::Text { text } => json!({
                            "sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": text}}),
                        Block::ToolCall { id, name, input } => {
                            let (locations, _) = preview(name, input, cwd, &mut |_| None);
                            json!({"sessionUpdate": "tool_call", "toolCallId": id,
                                   "title": title(name, input, cwd), "kind": kind(name),
                                   "status": "pending", "rawInput": input,
                                   "locations": locations})
                        }
                        _ => continue,
                    };
                    peer.update(sid, update);
                }
            }
        }
    }
}

/// One prompt's link to the client.
struct Turn<'a> {
    peer: &'a Peer,
    session: String,
    cwd: PathBuf,
    /// A call shown for a permission question, before it starts.
    pending: Option<String>,
    /// The running call and its kind.
    current: Option<(String, &'static str)>,
    files: Editor,
    stop: Option<Stop>,
    /// Text was sent this turn, and a tool call has run since: the client
    /// joins chunks into one message, so the next reply starts a paragraph.
    spoke: bool,
    gap: bool,
    /// The model's context window, for `usage_update`.
    window: u64,
    /// Tokens this turn's requests used.
    spent: Usage,
    /// The session's model and speed, which price each request.
    model: Model,
    fast: bool,
    /// What the session has cost, with this turn's requests so far.
    cost: crate::cost::Spend,
    /// Sub-agents by the model's id for their Agent call.
    agents: HashMap<String, Sub>,
}

/// A sub-agent's Agent call, and the steps it has taken.
struct Sub {
    id: String,
    label: String,
    steps: Vec<String>,
    tools: usize,
}

/// Steps kept in each update: the client gets the latest ones and the count.
const SUB_STEPS: usize = 20;

impl Sub {
    /// The sub-agent as `_meta.axio.subagent`, for clients that show it.
    fn meta(&self, status: &str, summary: Option<&str>) -> Value {
        let steps: Vec<Value> = self.steps.iter().map(|t| json!({"title": t})).collect();
        let mut v = json!({"label": self.label, "status": status, "steps": steps,
                           "tools": self.tools});
        if let Some(s) = summary {
            v["summary"] = json!(s);
        }
        json!({"axio": {"subagent": v}})
    }
}

impl Turn<'_> {
    fn update(&self, v: Value) {
        self.peer.update(&self.session, v);
    }

    fn new_call(&self) -> String {
        format!(
            "call_{}",
            self.peer.next_call.fetch_add(1, Ordering::Relaxed) + 1
        )
    }
}

impl Ui for Turn<'_> {
    fn delta(&mut self, d: Delta) {
        let (kind, text) = match d {
            Delta::Text(t) => {
                if self.gap && self.spoke {
                    self.update(json!({"sessionUpdate": "agent_message_chunk",
                                       "content": {"type": "text", "text": "\n\n"}}));
                }
                self.gap = false;
                self.spoke = true;
                ("agent_message_chunk", t)
            }
            Delta::ThinkingText(t) => ("agent_thought_chunk", t),
            _ => return,
        };
        self.update(json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}));
    }

    fn tool_start(&mut self, name: &str, input: &Value) {
        self.gap = true;
        let (locations, content) = if self.files.read {
            let mut files = Buffers {
                peer: self.peer,
                session: &self.session,
            };
            preview(name, input, &self.cwd, &mut |p| {
                crate::tools::Files::read(&mut files, p)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
            })
        } else {
            preview(name, input, &self.cwd, &mut disk_text)
        };
        let mut v = json!({"title": title(name, input, &self.cwd), "kind": kind(name),
                           "status": "in_progress", "rawInput": input,
                           "locations": locations, "content": content});
        let id = match self.pending.take() {
            Some(id) => {
                v["sessionUpdate"] = json!("tool_call_update");
                id
            }
            None => {
                v["sessionUpdate"] = json!("tool_call");
                self.new_call()
            }
        };
        v["toolCallId"] = json!(id);
        self.update(v);
        if name == "TodoWrite" {
            let entries: Vec<Value> = input["todos"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| {
                    json!({"content": t["content"], "priority": "medium",
                               "status": t["status"].as_str().unwrap_or("pending")})
                })
                .collect();
            self.update(json!({"sessionUpdate": "plan", "entries": entries}));
        }
        self.current = Some((id, kind(name)));
    }

    fn tool_end(&mut self, result: &Result<String, String>) {
        let Some((id, kind)) = self.current.take() else {
            return;
        };
        let mut v = json!({"sessionUpdate": "tool_call_update", "toolCallId": id});
        match result {
            Ok(out) => {
                v["status"] = json!("completed");
                // A Write or Edit keeps its diff; the rest show their output.
                if kind != "edit" {
                    v["content"] = text_content(out);
                }
            }
            Err(e) => {
                v["status"] = json!("failed");
                v["content"] = text_content(e);
            }
        }
        self.update(v);
    }

    fn stopped(&mut self, stop: &Stop) {
        self.stop = Some(stop.clone());
    }

    /// The call stays open after `tool_start`: the sub-agent's steps and its
    /// answer arrive later as updates to it.
    fn agent_planned(&mut self, call: &str, label: &str) {
        if let Some((id, _)) = self.current.take() {
            let sub = Sub {
                id,
                label: label.to_string(),
                steps: Vec::new(),
                tools: 0,
            };
            self.agents.insert(call.to_string(), sub);
        }
    }

    fn agent_step(&mut self, call: &str, title: &str) {
        let Some(sub) = self.agents.get_mut(call) else {
            return;
        };
        sub.tools += 1;
        sub.steps.push(title.to_string());
        if sub.steps.len() > SUB_STEPS {
            sub.steps.remove(0);
        }
        let v = json!({"sessionUpdate": "tool_call_update", "toolCallId": sub.id,
                       "status": "in_progress", "_meta": sub.meta("running", None)});
        self.update(v);
    }

    fn agent_done(
        &mut self,
        call: &str,
        _label: &str,
        result: &Result<String, String>,
        summary: &str,
    ) {
        let Some(sub) = self.agents.get(call) else {
            return;
        };
        let (status, text) = match result {
            Ok(t) => ("completed", t),
            Err(e) => ("failed", e),
        };
        let v = json!({"sessionUpdate": "tool_call_update", "toolCallId": sub.id,
                       "status": status, "content": text_content(text),
                       "_meta": sub.meta(status, Some(summary))});
        self.update(v);
    }

    /// After each request: how full the context is, and what the turn used.
    fn usage(&mut self, u: Usage) {
        self.spent.input += u.input;
        self.spent.output += u.output;
        self.spent.cache_read += u.cache_read;
        self.spent.cache_write += u.cache_write;
        self.cost.add(&self.model, self.fast, &u);
        let mut update =
            json!({"sessionUpdate": "usage_update", "used": u.context(), "size": self.window});
        // Only a whole price: none when some tokens had no price or a plan
        // paid for them.
        if self.cost.unpriced_tokens == 0 && !self.cost.on_plan && self.cost.usd > 0.0 {
            update["cost"] = json!({"amount": self.cost.usd, "currency": "USD"});
        }
        self.update(update);
    }

    fn interrupted(&mut self) -> bool {
        self.peer.stopped()
    }

    fn notice(&mut self, text: &str) {
        self.update(json!({"sessionUpdate": "agent_thought_chunk",
                           "content": {"type": "text", "text": format!("[axio] {text}\n")}}));
    }

    fn can_ask(&self) -> bool {
        true
    }

    /// Read, Write and Edit go through the client when it offers files, so
    /// they see unsaved buffers and the editor sees the changes.
    fn remote_tool(
        &mut self,
        name: &str,
        input: &Value,
    ) -> Option<(Result<String, String>, Vec<Block>)> {
        let path = input["file_path"].as_str()?;
        let through = match name {
            "Read" => self.files.read && crate::context::image_type(Path::new(path)).is_none(),
            "Write" => self.files.write,
            "Edit" => self.files.read && self.files.write,
            _ => false,
        };
        if !through {
            return None;
        }
        let mut files = Buffers {
            peer: self.peer,
            session: &self.session,
        };
        Some((
            crate::tools::run_files(name, input, &self.cwd, &mut files),
            Vec::new(),
        ))
    }

    fn ask(&mut self, question: &str) -> bool {
        self.permission(question, &["Yes".into(), "No".into()]) == Some(0)
    }

    fn permission(&mut self, question: &str, options: &[String]) -> Option<usize> {
        let id = match &self.pending {
            Some(id) => id.clone(),
            None => {
                let id = self.new_call();
                self.update(json!({"sessionUpdate": "tool_call", "toolCallId": id,
                                   "title": question, "status": "pending"}));
                self.pending = Some(id.clone());
                id
            }
        };
        let last = options.len().saturating_sub(1);
        let opts: Vec<Value> = options
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let kind = match i {
                    0 => "allow_once",
                    _ if i == last && name.starts_with("No") => "reject_once",
                    _ => "allow_always",
                };
                json!({"optionId": i.to_string(), "name": name, "kind": kind})
            })
            .collect();
        let reply = self.peer.request(
            "session/request_permission",
            json!({"sessionId": self.session,
                   "toolCall": {"toolCallId": id, "title": question, "status": "pending"},
                   "options": opts}),
        )?;
        let outcome = &reply["result"]["outcome"];
        if outcome["outcome"] != "selected" {
            return None;
        }
        outcome["optionId"].as_str()?.parse().ok()
    }
}

/// Files read and written through the client.
struct Buffers<'a> {
    peer: &'a Peer,
    session: &'a str,
}

impl Buffers<'_> {
    fn ask(&self, method: &str, mut params: Value, path: &Path) -> Result<Value, String> {
        params["sessionId"] = json!(self.session);
        params["path"] = json!(path.display().to_string());
        let r = self
            .peer
            .request(method, params)
            .ok_or_else(|| crate::tools::STOPPED.to_string())?;
        match r.get("error") {
            Some(e) => Err(format!(
                "{}: {}",
                path.display(),
                e["message"].as_str().unwrap_or("the editor refused")
            )),
            None => Ok(r["result"].clone()),
        }
    }
}

impl crate::tools::Files for Buffers<'_> {
    fn read(&mut self, path: &Path) -> Result<Vec<u8>, String> {
        let r = self.ask("fs/read_text_file", json!({}), path)?;
        Ok(r["content"].as_str().unwrap_or("").as_bytes().to_vec())
    }
    fn write(&mut self, path: &Path, text: &str) -> Result<(), String> {
        self.ask("fs/write_text_file", json!({"content": text}), path)
            .map(|_| ())
    }
}

/// Saved sessions, newest first: one folder's when `cwd` is given, else all.
fn list(home: &Path, params: &Value) -> Value {
    const PAGE: usize = 50;
    let dirs: Vec<PathBuf> = match params["cwd"].as_str() {
        Some(c) => vec![crate::session::dir_for(home, Path::new(c))],
        None => std::fs::read_dir(home.join("sessions"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
    };
    let mut rows: Vec<(std::time::SystemTime, Value)> = Vec::new();
    for dir in dirs {
        for s in crate::session::recent(&dir, usize::MAX) {
            let Some(cwd) = header_cwd(&s.path) else {
                continue;
            };
            let title: String = s
                .first_prompt
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(80)
                .collect();
            rows.push((
                s.modified,
                json!({"sessionId": session_id(&s.path), "cwd": cwd, "title": title,
                       "updatedAt": crate::context::iso_time(s.modified)}),
            ));
        }
    }
    rows.sort_by_key(|a| std::cmp::Reverse(a.0));
    let start: usize = params["cursor"]
        .as_str()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let page: Vec<Value> = rows
        .iter()
        .skip(start)
        .take(PAGE)
        .map(|(_, v)| v.clone())
        .collect();
    let mut out = json!({ "sessions": page });
    if start + PAGE < rows.len() {
        out["nextCursor"] = json!((start + PAGE).to_string());
    }
    out
}

/// The folder a session was started in, from its header line.
fn header_cwd(path: &Path) -> Option<String> {
    use std::io::BufRead;
    let mut first = String::new();
    std::io::BufReader::new(std::fs::File::open(path).ok()?)
        .read_line(&mut first)
        .ok()?;
    let v: Value = serde_json::from_str(&first).ok()?;
    Some(v["cwd"].as_str()?.to_string())
}

/// For hooks run outside a prompt (SessionStart, SessionEnd).
struct Quiet;

impl Ui for Quiet {
    fn delta(&mut self, _: Delta) {}
    fn tool_start(&mut self, _: &str, _: &Value) {}
    fn tool_end(&mut self, _: &Result<String, String>) {}
    fn stopped(&mut self, _: &Stop) {}
    fn usage(&mut self, _: Usage) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Reply, Request};
    use std::time::Instant;

    #[derive(Clone)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    type Script = fn(&Request) -> Reply;

    /// A client wired to a server on its own thread, as stdin would be.
    struct Client {
        peer: Arc<Peer>,
        to_server: Option<Sender<Value>>,
        out: Buf,
        server: Option<std::thread::JoinHandle<()>>,
        next: u64,
    }

    impl Client {
        fn start(dir: &Path, script: Script) -> Client {
            let out = Buf(Arc::new(Mutex::new(Vec::new())));
            let peer = Peer::new(Box::new(out.clone()));
            let (tx, rx) = channel();
            let (p, sessions, home) = (peer.clone(), dir.to_path_buf(), dir.to_path_buf());
            let server = std::thread::spawn(move || {
                let make: Make = Box::new(move |cwd, resume, _| {
                    let mut a = Agent::new(
                        Model::parse("anthropic/claude-opus-5-5"),
                        cwd.to_path_buf(),
                        String::new(),
                    );
                    a.set_tools(crate::tools::Modules::default(), crate::mcp::Mcp::default());
                    a.max_retries = 0;
                    a.set_permissions(crate::permissions::Permissions::default());
                    let sessions = crate::session::dir_for(&sessions, cwd);
                    match resume {
                        Some(id) => {
                            let path = sessions.join(format!("{id}.jsonl"));
                            a.messages = crate::session::load(&path)?;
                            a.session = Some(crate::session::Session::open(path)?);
                        }
                        None => a.session = Some(crate::session::Session::create(&sessions, cwd)?),
                    }
                    a.set_complete(move |req, on| {
                        let r = script(req);
                        for b in &r.content {
                            if let Block::Text { text } = b {
                                on(Delta::Text(text));
                            }
                        }
                        Ok(r)
                    });
                    Ok((a, Library::default()))
                });
                Server::new(p, make, home).run(rx);
            });
            Client {
                peer,
                to_server: Some(tx),
                out,
                server: Some(server),
                next: 0,
            }
        }

        fn send(&self, msg: Value) {
            if let Some(m) = self.peer.route(msg) {
                self.to_server.as_ref().unwrap().send(m).unwrap();
            }
        }

        /// Whole lines only: the server may be partway through writing one.
        fn lines(&self) -> Vec<Value> {
            let out = self.out.0.lock().unwrap().clone();
            let done = out.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
            String::from_utf8_lossy(&out[..done])
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }

        fn wait(&self, what: &str, f: impl Fn(&Value) -> bool) -> Value {
            let start = Instant::now();
            loop {
                if let Some(v) = self.lines().into_iter().find(|v| f(v)) {
                    return v;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "no {what} in {:#?}",
                    self.lines()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// Sends a request and waits for its response.
        fn call(&mut self, method: &str, params: Value) -> Value {
            self.next += 1;
            let id = self.next;
            self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
            self.wait(method, |v| v["id"] == id && v.get("method").is_none())
        }

        fn updates(&self, kind: &str) -> Vec<Value> {
            self.lines()
                .into_iter()
                .filter(|v| v["method"] == "session/update")
                .map(|v| v["params"]["update"].clone())
                .filter(|u| u["sessionUpdate"] == kind)
                .collect()
        }

        fn new_session(&mut self, cwd: &Path) -> String {
            let r = self.call("session/new", json!({"cwd": cwd, "mcpServers": []}));
            r["result"]["sessionId"].as_str().unwrap().to_string()
        }
    }

    impl Drop for Client {
        fn drop(&mut self) {
            self.peer.close();
            self.to_server.take();
            if let Some(s) = self.server.take() {
                let _ = s.join();
            }
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axio-acp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn reply(stop: Stop, content: Vec<Block>) -> Reply {
        Reply {
            stop,
            content,
            usage: Usage::default(),
        }
    }

    fn text(t: &str) -> Block {
        Block::Text { text: t.into() }
    }

    fn call(name: &str, input: Value) -> Block {
        Block::ToolCall {
            id: "t1".into(),
            name: name.into(),
            input,
        }
    }

    /// Calls `tool` for each prompt, then says "done".
    fn once(req: &Request, tool: Block) -> Reply {
        let answered = matches!(req.messages.last(), Some(Message::User { content })
            if matches!(content.last(), Some(Block::ToolResult { .. })));
        if !answered {
            reply(Stop::ToolUse, vec![tool])
        } else {
            reply(Stop::EndTurn, vec![text("done")])
        }
    }

    #[test]
    fn initialize_reports_version_and_capabilities() {
        let dir = temp("init");
        let mut c = Client::start(&dir, |_| reply(Stop::EndTurn, vec![]));
        let r = c.call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        );
        let r = &r["result"];
        assert_eq!(r["protocolVersion"], 1);
        assert_eq!(r["agentCapabilities"]["loadSession"], true);
        assert_eq!(r["agentCapabilities"]["promptCapabilities"]["image"], true);
        assert_eq!(r["agentInfo"]["name"], "axio");
        let r = c.call("no/such", json!({}));
        assert_eq!(r["error"]["code"], NOT_FOUND);
    }

    #[test]
    fn a_prompt_streams_text_and_tool_calls_then_ends_the_turn() {
        let dir = temp("prompt");
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        let mut c = Client::start(&dir, |req| {
            once(req, call("Read", json!({"file_path": "notes.txt"})))
        });
        let sid = c.new_session(&dir);
        assert!(valid_id(&sid), "{sid}");
        assert_eq!(c.updates("available_commands_update").len(), 1);
        let r = c.call(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "read it"}]}),
        );
        assert_eq!(r["result"]["stopReason"], "end_turn");
        let start = &c.updates("tool_call")[0];
        assert_eq!(start["kind"], "read");
        assert_eq!(start["status"], "in_progress");
        assert_eq!(start["title"], "Read notes.txt");
        assert_eq!(
            start["locations"][0]["path"],
            dir.join("notes.txt").display().to_string()
        );
        let end = &c.updates("tool_call_update")[0];
        assert_eq!(end["toolCallId"], start["toolCallId"]);
        assert_eq!(end["status"], "completed");
        assert!(
            end["content"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("hello")
        );
        let said = c.updates("agent_message_chunk");
        assert_eq!(said.last().unwrap()["content"]["text"], "done");
    }

    #[test]
    fn replies_either_side_of_a_tool_call_stay_apart() {
        let dir = temp("gap");
        let mut c = Client::start(&dir, |req| {
            let mut r = once(req, call("Bash", json!({"command": "true"})));
            if r.stop == Stop::ToolUse {
                r.content.insert(0, text("Let me check:"));
            }
            r
        });
        let sid = c.new_session(&dir);
        c.call(
            "session/set_mode",
            json!({"sessionId": sid, "modeId": "bypassPermissions"}),
        );
        c.call(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "go"}]}),
        );
        let said: String = c
            .updates("agent_message_chunk")
            .iter()
            .map(|u| u["content"]["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(said, "Let me check:\n\ndone");
    }

    #[test]
    fn a_saved_session_loads_and_replays() {
        let dir = temp("load");
        let sid = {
            let mut c = Client::start(&dir, |_| reply(Stop::EndTurn, vec![text("hi there")]));
            let sid = c.new_session(&dir);
            c.call(
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type": "text", "text": "hello"}]}),
            );
            sid
        };
        let mut c = Client::start(&dir, |_| reply(Stop::EndTurn, vec![]));
        let r = c.call(
            "session/load",
            json!({"sessionId": sid, "cwd": dir, "mcpServers": []}),
        );
        assert!(r.get("error").is_none(), "{r}");
        assert_eq!(r["result"]["modes"]["currentModeId"], "default");
        let user = c.updates("user_message_chunk");
        assert_eq!(user[0]["content"]["text"], "hello");
        assert_eq!(
            c.updates("agent_message_chunk")[0]["content"]["text"],
            "hi there"
        );
        let bad = c.call(
            "session/load",
            json!({"sessionId": "../etc", "cwd": dir, "mcpServers": []}),
        );
        assert_eq!(bad["error"]["code"], INVALID_PARAMS);
        let rel = c.call("session/new", json!({"cwd": "relative", "mcpServers": []}));
        assert_eq!(rel["error"]["code"], INVALID_PARAMS);
    }

    /// Says how many messages the request carried.
    fn count(req: &Request) -> Reply {
        reply(
            Stop::EndTurn,
            vec![text(&format!("saw {}", req.messages.len()))],
        )
    }

    #[test]
    fn a_session_forks_into_another_folder_with_its_history() {
        let dir = temp("fork");
        let side = dir.join("side");
        std::fs::create_dir_all(&side).unwrap();
        let sid = {
            let mut c = Client::start(&dir, |_| reply(Stop::EndTurn, vec![text("hi there")]));
            let sid = c.new_session(&dir);
            c.call(
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type": "text", "text": "hello"}]}),
            );
            sid
        };
        let mut c = Client::start(&dir, count);
        let init = c.call("initialize", json!({"protocolVersion": 1}));
        assert!(init["result"]["agentCapabilities"]["sessionCapabilities"]["fork"].is_object());
        let r = c.call(
            "session/fork",
            json!({"sessionId": sid, "cwd": side, "mcpServers": []}),
        );
        assert!(r.get("error").is_none(), "{r}");
        let fork = r["result"]["sessionId"].as_str().unwrap().to_string();
        assert_ne!(fork, sid);
        assert!(
            c.updates("user_message_chunk").is_empty(),
            "a fork does not replay"
        );
        c.call(
            "session/prompt",
            json!({"sessionId": fork, "prompt": [{"type": "text", "text": "and then?"}]}),
        );
        assert_eq!(
            c.updates("agent_message_chunk").last().unwrap()["content"]["text"],
            "saw 3",
            "the fork's first prompt carries the parent's turn"
        );
        // The parent goes on as it was.
        let parent = crate::session::dir_for(&dir, &dir).join(format!("{sid}.jsonl"));
        let kept = std::fs::read_to_string(parent).unwrap();
        assert!(!kept.contains("and then?"), "{kept}");
        let missing = c.call(
            "session/fork",
            json!({"sessionId": "no-such-session", "cwd": side, "mcpServers": []}),
        );
        assert_eq!(missing["error"]["code"], INVALID_PARAMS);
    }

    fn costly(_: &Request) -> Reply {
        Reply {
            stop: Stop::EndTurn,
            content: vec![text("ok")],
            usage: Usage {
                input: 100,
                output: 20,
                cache_read: 30,
                cache_write: 0,
            },
        }
    }

    #[test]
    fn a_turn_reports_its_context_and_the_sessions_tokens() {
        let dir = temp("usage");
        let mut c = Client::start(&dir, costly);
        let sid = c.new_session(&dir);
        let ask = |c: &mut Client| {
            c.call(
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type": "text", "text": "go"}]}),
            )
        };
        let first = ask(&mut c);
        let used = c.updates("usage_update");
        assert_eq!(used.last().unwrap()["used"], 150);
        assert_eq!(
            used.last().unwrap()["size"],
            Model::parse("anthropic/claude-opus-5-5").context_window()
        );
        let usage = &first["result"]["usage"];
        assert_eq!(
            (
                usage["totalTokens"].as_u64(),
                usage["inputTokens"].as_u64(),
                usage["outputTokens"].as_u64()
            ),
            (Some(150), Some(130), Some(20))
        );
        assert_eq!(usage["cachedReadTokens"], 30);
        // Totals run on across the session's turns.
        // What it cost, priced as the terminal prices it, running on too.
        let cost = |c: &Client| c.updates("usage_update").last().unwrap()["cost"].clone();
        assert_eq!(cost(&c)["currency"], "USD");
        let amount = |c: &Client| cost(c)["amount"].as_f64().unwrap();
        assert!((amount(&c) - 0.000806).abs() < 1e-12, "{}", amount(&c));
        let second = ask(&mut c);
        assert_eq!(second["result"]["usage"]["totalTokens"], 300);
        assert!((amount(&c) - 0.001612).abs() < 1e-12, "{}", amount(&c));
    }

    fn write_call(_: &Request) -> Block {
        call("Write", json!({"file_path": "out.txt", "content": "new"}))
    }

    /// Answers the next permission request with option `choice`.
    fn answer(c: &Client, nth: usize, choice: Option<&str>) -> Value {
        let ask = c.wait("permission request", |v| {
            v["method"] == "session/request_permission" && v["id"] == nth as u64
        });
        let outcome = match choice {
            Some(o) => json!({"outcome": "selected", "optionId": o}),
            None => json!({"outcome": "cancelled"}),
        };
        c.send(json!({"jsonrpc": "2.0", "id": ask["id"], "result": {"outcome": outcome}}));
        ask
    }

    #[test]
    fn writes_ask_the_client_and_follow_its_answer() {
        let dir = temp("permission");
        std::fs::write(dir.join("out.txt"), "old").unwrap();
        let mut c = Client::start(&dir, |req| once(req, write_call(req)));
        let sid = c.new_session(&dir);
        let prompt = json!({"sessionId": sid, "prompt": [{"type": "text", "text": "write"}]});
        c.next += 1;
        c.send(
            json!({"jsonrpc": "2.0", "id": c.next, "method": "session/prompt", "params": prompt}),
        );
        let ask = answer(&c, 1, Some("2"));
        let opts = &ask["params"]["options"];
        let kinds: Vec<&str> = opts
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["allow_once", "allow_always", "reject_once"]);
        let id = c.next;
        let done = c.wait("prompt result", |v| {
            v["id"] == id && v.get("method").is_none()
        });
        assert_eq!(done["result"]["stopReason"], "end_turn");
        assert_eq!(std::fs::read_to_string(dir.join("out.txt")).unwrap(), "old");
        let shown = &c.updates("tool_call")[0];
        assert_eq!(shown["status"], "pending");
        assert_eq!(shown["toolCallId"], ask["params"]["toolCall"]["toolCallId"]);
        let failed = c.updates("tool_call_update");
        assert_eq!(failed.last().unwrap()["status"], "failed");

        // Yes, and accept edits: the file changes, the mode moves.
        c.next += 1;
        c.send(
            json!({"jsonrpc": "2.0", "id": c.next, "method": "session/prompt", "params": prompt}),
        );
        answer(&c, 2, Some("1"));
        let id = c.next;
        c.wait("prompt result", |v| {
            v["id"] == id && v.get("method").is_none()
        });
        assert_eq!(std::fs::read_to_string(dir.join("out.txt")).unwrap(), "new");
        let diff = c
            .updates("tool_call_update")
            .into_iter()
            .find(|u| u["status"] == "in_progress")
            .unwrap();
        assert_eq!(diff["content"][0]["type"], "diff");
        assert_eq!(diff["content"][0]["oldText"], "old");
        assert_eq!(diff["content"][0]["newText"], "new");
        let mode = c.updates("current_mode_update");
        assert_eq!(mode[0]["currentModeId"], "acceptEdits");
    }

    #[test]
    fn cancel_stops_a_waiting_prompt() {
        let dir = temp("cancel");
        let mut c = Client::start(&dir, |req| once(req, write_call(req)));
        let sid = c.new_session(&dir);
        c.next += 1;
        let id = c.next;
        c.send(
            json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
                      "params": {"sessionId": sid, "prompt": [{"type": "text", "text": "w"}]}}),
        );
        c.wait("permission request", |v| {
            v["method"] == "session/request_permission"
        });
        c.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}}));
        let done = c.wait("prompt result", |v| {
            v["id"] == id && v.get("method").is_none()
        });
        assert_eq!(done["result"]["stopReason"], "cancelled");
        assert!(!dir.join("out.txt").exists());
        // The next prompt runs normally.
        let r = c.call(
            "session/set_mode",
            json!({"sessionId": sid, "modeId": "bypassPermissions"}),
        );
        assert!(r.get("error").is_none(), "{r}");
        let r = c.call(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "again"}]}),
        );
        assert_eq!(r["result"]["stopReason"], "end_turn");
        assert!(dir.join("out.txt").exists(), "{:#?}", c.lines());
    }

    #[test]
    fn config_options_set_model_effort_and_mode() {
        let dir = temp("config");
        let mut c = Client::start(&dir, |_| reply(Stop::EndTurn, vec![]));
        let r = c.call("session/new", json!({"cwd": dir, "mcpServers": []}));
        let sid = r["result"]["sessionId"].clone();
        let ids: Vec<&str> = r["result"]["configOptions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["model", "effort", "mode"]);
        let set = |c: &mut Client, key: &str, value: &str| {
            c.call(
                "session/set_config_option",
                json!({"sessionId": sid, "configId": key, "value": value}),
            )
        };
        let r = set(&mut c, "effort", "low");
        let effort = &r["result"]["configOptions"][1];
        assert_eq!(effort["currentValue"], "low");
        assert_eq!(
            set(&mut c, "effort", "ludicrous")["error"]["code"],
            INVALID_PARAMS
        );
        let r = set(&mut c, "mode", "plan");
        assert_eq!(r["result"]["configOptions"][2]["currentValue"], "plan");
        let r = set(&mut c, "model", "openai/gpt-test");
        assert_eq!(
            r["result"]["configOptions"][0]["currentValue"],
            "openai/gpt-test"
        );
        assert_eq!(
            set(&mut c, "colour", "red")["error"]["code"],
            INVALID_PARAMS
        );
        c.send(json!({"method": "_axio/models", "params": {"models": ["anthropic/a", "openai/gpt-test"]}}));
        let pushed = c.wait("model list", |v| {
            v["params"]["update"]["sessionUpdate"] == "config_option_update"
        });
        let values: Vec<&str> = pushed["params"]["update"]["configOptions"][0]["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["value"].as_str().unwrap())
            .collect();
        assert_eq!(values, ["openai/gpt-test", "anthropic/a"]);
    }

    #[test]
    fn files_go_through_an_editor_that_offers_them() {
        let dir = temp("clientfs");
        std::fs::write(dir.join("notes.txt"), "ON DISK").unwrap();
        let mut c = Client::start(&dir, |req| {
            let tool = match req.messages.len() {
                1 => call("Read", json!({"file_path": "notes.txt"})),
                3 => call(
                    "Write",
                    json!({"file_path": "out.txt", "content": "via editor"}),
                ),
                5 => call(
                    "Edit",
                    json!({"file_path": "notes.txt", "old_string": "UNSAVED", "new_string": "EDITED"}),
                ),
                _ => return reply(Stop::EndTurn, vec![text("done")]),
            };
            reply(Stop::ToolUse, vec![tool])
        });
        c.call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities":
                   {"fs": {"readTextFile": true, "writeTextFile": true}}}),
        );
        let sid = c.new_session(&dir);
        c.call(
            "session/set_mode",
            json!({"sessionId": sid, "modeId": "bypassPermissions"}),
        );
        c.next += 1;
        let id = c.next;
        c.send(
            json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
                      "params": {"sessionId": sid, "prompt": [{"type": "text", "text": "go"}]}}),
        );
        // Answer every file request as an editor would, until the turn ends.
        let mut answered = Vec::new();
        let mut writes = Vec::new();
        let done = loop {
            for v in c.lines() {
                let m = v["method"].as_str().unwrap_or("");
                if !m.starts_with("fs/") || answered.contains(&v["id"]) {
                    continue;
                }
                answered.push(v["id"].clone());
                assert_eq!(v["params"]["sessionId"], sid.as_str());
                let result = if m == "fs/read_text_file" {
                    json!({"content": "UNSAVED BUFFER"})
                } else {
                    writes.push(v["params"]["content"].clone());
                    json!({})
                };
                c.send(json!({"jsonrpc": "2.0", "id": v["id"], "result": result}));
            }
            let finished = |v: &Value| v["id"] == id && v.get("method").is_none();
            if let Some(r) = c.lines().into_iter().find(finished) {
                break r;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(writes, [json!("via editor"), json!("EDITED BUFFER")]);
        assert_eq!(done["result"]["stopReason"], "end_turn");
        let edit = c
            .updates("tool_call")
            .into_iter()
            .find(|u| u["title"] == "Edit notes.txt")
            .unwrap();
        assert_eq!(
            edit["content"][0]["oldText"], "UNSAVED BUFFER",
            "diff from the buffer"
        );
        let shown = c
            .updates("tool_call_update")
            .into_iter()
            .find(|u| u["status"] == "completed")
            .unwrap();
        assert!(
            shown["content"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("UNSAVED BUFFER")
        );
        assert!(!dir.join("out.txt").exists(), "the editor writes, not axio");
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
            "ON DISK"
        );
    }

    #[test]
    fn without_editor_files_axio_uses_the_disk() {
        let dir = temp("diskfs");
        std::fs::write(dir.join("notes.txt"), "ON DISK").unwrap();
        let mut c = Client::start(&dir, |req| {
            once(req, call("Read", json!({"file_path": "notes.txt"})))
        });
        c.call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        );
        let sid = c.new_session(&dir);
        c.call(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "go"}]}),
        );
        let shown = &c.updates("tool_call_update")[0];
        assert!(
            shown["content"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("ON DISK")
        );
        assert!(c.lines().iter().all(|v| v["method"] != "fs/read_text_file"));
    }

    #[test]
    fn sessions_list_newest_first() {
        let dir = temp("list");
        let mut c = Client::start(&dir, |_| reply(Stop::EndTurn, vec![text("ok")]));
        let r = c.call("initialize", json!({"protocolVersion": 1}));
        assert!(r["result"]["agentCapabilities"]["sessionCapabilities"]["list"].is_object());
        let first = c.new_session(&dir);
        c.call(
            "session/prompt",
            json!({"sessionId": first, "prompt": [{"type": "text", "text": "first task\nmore"}]}),
        );
        std::thread::sleep(Duration::from_millis(20));
        let second = c.new_session(&dir);
        c.call(
            "session/prompt",
            json!({"sessionId": second, "prompt": [{"type": "text", "text": "second task"}]}),
        );
        let r = c.call("session/list", json!({"cwd": dir}));
        let rows = r["result"]["sessions"].as_array().unwrap();
        let ids: Vec<&str> = rows
            .iter()
            .map(|s| s["sessionId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [second.as_str(), first.as_str()]);
        assert_eq!(rows[1]["title"], "first task");
        assert_eq!(rows[1]["cwd"], dir.display().to_string());
        let at = rows[0]["updatedAt"].as_str().unwrap();
        assert!(
            at.len() == 20 && at.ends_with('Z') && at.contains('T'),
            "{at}"
        );
        let all = c.call("session/list", json!({}));
        assert_eq!(all["result"]["sessions"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn client_mcp_servers_become_mcp_json() {
        let doc = mcp_doc(&json!([
            {"name": "files", "command": "/bin/fs", "args": ["-v"],
             "env": [{"name": "TOKEN", "value": "x"}]},
            {"type": "http", "name": "web", "url": "https://e.test/mcp",
             "headers": [{"name": "Authorization", "value": "Bearer y"}]}
        ]));
        assert_eq!(
            doc,
            json!({"mcpServers": {
                "files": {"command": "/bin/fs", "args": ["-v"], "env": {"TOKEN": "x"}},
                "web": {"type": "http", "url": "https://e.test/mcp",
                        "headers": {"Authorization": "Bearer y"}}
            }})
        );
    }

    #[test]
    fn prompt_blocks_become_text_mentions_and_attachments() {
        let (text, attached) = content(&json!([
            {"type": "text", "text": "look at"},
            {"type": "resource_link", "uri": "file:///w/my%20file.rs", "name": "my file.rs"},
            {"type": "text", "text": "and this"},
            {"type": "resource", "resource": {"uri": "file:///w/a.rs", "text": "fn a() {}"}},
            {"type": "image", "mimeType": "image/png", "data": "AAAA"},
            {"type": "audio", "mimeType": "audio/wav", "data": "AAAA"}
        ]));
        assert_eq!(text, "look at @\"/w/my file.rs\" and this");
        assert_eq!(attached.len(), 2);
        assert!(matches!(&attached[0], Block::Text { text } if text.contains("fn a() {}")));
        assert!(
            matches!(&attached[1], Block::Image { media_type, .. } if media_type == "image/png")
        );
    }
}
