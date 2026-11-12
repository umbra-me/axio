//! MCP client over stdio: axio starts each server, speaks newline-delimited
//! JSON-RPC 2.0 on its stdin and stdout, and offers its tools to the model as
//! `mcp__<server>__<tool>`.

use crate::message::Block;
use crate::tools::ToolSpec;

/// The largest image passed to the model (base64 makes it a third larger,
/// and providers take up to 5 MB).
const MAX_IMAGE_BYTES: usize = 3_750_000;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PROTOCOL: &str = "2025-06-18";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Anthropic and OpenAI both cap tool names at 64 characters.
const NAME_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct ServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    /// Set for servers reached over streamable HTTP instead of a process.
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    /// The older HTTP+SSE transport: `url` is the event stream.
    pub sse: bool,
}

/// `${VAR}` from the environment, so tokens stay out of config files.
fn expand_env(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(end) => {
                let name = &rest[i + 2..i + end];
                out.push_str(&std::env::var(name).unwrap_or_default());
                rest = &rest[i + end + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Servers from `{"mcpServers": {...}}` documents. Entries axio cannot run
/// yet (HTTP and SSE transports) or that are disabled are reported, not
/// started.
#[derive(Debug, Default)]
pub struct Config {
    pub servers: Vec<ServerConfig>,
    pub skipped: Vec<String>,
}

fn expand(text: &str, root: Option<&Path>) -> String {
    match root {
        Some(r) => {
            let r = r.display().to_string();
            text.replace("${CLAUDE_PLUGIN_ROOT}", &r)
                .replace("${AXIO_PLUGIN_ROOT}", &r)
        }
        None => text.to_string(),
    }
}

impl Config {
    pub fn add(&mut self, doc: &Value, root: Option<&Path>, source: &Path) {
        let Some(servers) = doc.get("mcpServers").and_then(Value::as_object) else {
            return;
        };
        for (name, s) in servers {
            if s["disabled"] == true || s["enabled"] == false {
                continue;
            }
            let kind = s["type"].as_str().unwrap_or(if s.get("url").is_some() {
                "http"
            } else {
                "stdio"
            });
            let url = match kind {
                "http" | "streamable-http" | "sse" => match s["url"].as_str() {
                    Some(u) => Some(expand_env(&expand(u, root))),
                    None => {
                        self.skipped
                            .push(format!("{}: {name}: it has no url", source.display()));
                        continue;
                    }
                },
                "stdio" => None,
                other => {
                    self.skipped.push(format!(
                        "{}: {name}: unknown transport {other}",
                        source.display()
                    ));
                    continue;
                }
            };
            let command = s["command"].as_str().unwrap_or("");
            if url.is_none() && command.is_empty() {
                self.skipped
                    .push(format!("{}: {name}: it has no command", source.display()));
                continue;
            }
            if self.servers.iter().any(|x| &x.name == name) {
                self.skipped.push(format!(
                    "{}: {name}: a server with this name is already configured",
                    source.display()
                ));
                continue;
            }
            self.servers.push(ServerConfig {
                name: name.clone(),
                command: expand(command, root),
                args: s["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|a| expand(a, root))
                    .collect(),
                env: s["env"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(k, v)| Some((k.clone(), expand(v.as_str()?, root))))
                    .collect(),
                timeout: s["timeout"]
                    .as_u64()
                    .map_or(CALL_TIMEOUT, Duration::from_millis),
                headers: s["headers"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(k, v)| Some((k.clone(), expand_env(v.as_str()?))))
                    .collect(),
                url,
                sse: kind == "sse",
            });
        }
    }

    pub fn add_file(&mut self, path: &Path, root: Option<&Path>) -> Result<(), String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        self.add(&doc, root, path);
        Ok(())
    }
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;

struct StdioConn {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: u64,
}

impl Drop for StdioConn {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn send(stdin: &Mutex<ChildStdin>, msg: &Value) -> Result<(), String> {
    let mut s = stdin.lock().map_err(|_| "server pipe poisoned")?;
    writeln!(s, "{msg}")
        .and_then(|_| s.flush())
        .map_err(|e| format!("server closed its input: {e}"))
}

impl StdioConn {
    fn spawn(cfg: &ServerConfig, cwd: &Path) -> Result<StdioConn, String> {
        let mut child = Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(cfg.env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start `{}`: {e}", cfg.command))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let pending: Pending = Arc::default();
        let stdout = child.stdout.take().unwrap();
        let (reader_pending, reader_stdin, cwd) =
            (pending.clone(), stdin.clone(), cwd.to_path_buf());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // Servers sometimes log to stdout; anything that is not a
                // JSON-RPC object is ignored.
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if msg.get("method").is_some() {
                    if let Some(reply) = answer_server_request(&msg, &cwd) {
                        let _ = send(&reader_stdin, &reply);
                    }
                    continue;
                }
                let Some(id) = msg["id"].as_u64() else {
                    continue;
                };
                let result = match msg.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                    None => Ok(msg["result"].clone()),
                };
                if let Some(tx) = reader_pending.lock().ok().and_then(|mut p| p.remove(&id)) {
                    let _ = tx.send(result);
                }
            }
            // The server is gone: fail everything still waiting.
            if let Ok(mut p) = reader_pending.lock() {
                for (_, tx) in p.drain() {
                    let _ = tx.send(Err("the server exited".into()));
                }
            }
        });
        Ok(StdioConn {
            child,
            stdin,
            pending,
            next_id: 1,
        })
    }

    fn start(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(u64, Receiver<Result<Value, String>>), String> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = mpsc::channel();
        self.pending.lock().map_err(|_| "poisoned")?.insert(id, tx);
        send(
            &self.stdin,
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )?;
        Ok((id, rx))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        send(
            &self.stdin,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
    }
}

/// A streamable HTTP server: every message is a POST; replies come back as
/// JSON or as an event stream that may carry the server's own requests first.
struct HttpConn {
    url: String,
    headers: Vec<(String, String)>,
    session: Arc<Mutex<Option<String>>>,
    initialized: bool,
    next_id: u64,
    cwd: PathBuf,
}

const SIGN_IN: &str = "the server needs a sign-in";

fn sign_in_hint(name: &str) -> String {
    format!("{SIGN_IN}: run `axio mcp login {name}` (or set an Authorization header)")
}

/// `token_url` is the configured server URL a stored sign-in belongs to.
fn http_post(
    url: &str,
    token_url: &str,
    headers: &[(String, String)],
    session: &Mutex<Option<String>>,
    initialized: bool,
    msg: &Value,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let send = |token: Option<&str>| {
        let mut req = agent
            .post(url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if initialized {
            req = req.header("mcp-protocol-version", PROTOCOL);
        }
        if let Some(id) = session.lock().ok().and_then(|s| s.clone()) {
            req = req.header("mcp-session-id", &id);
        }
        for (k, v) in headers {
            req = req.header(k, v);
        }
        if let Some(t) = token {
            req = req.header("authorization", &format!("Bearer {t}"));
        }
        req.send(msg.to_string())
            .map_err(|e| format!("could not reach {url}: {e}"))
    };
    // A stored sign-in supplies the token unless the config sets its own.
    let own = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("authorization"));
    let token = if own {
        None
    } else {
        crate::oauth::bearer(token_url, false)
    };
    let mut resp = send(token.as_deref())?;
    if resp.status().as_u16() == 401
        && token.is_some()
        && let Some(fresh) = crate::oauth::bearer(token_url, true)
    {
        resp = send(Some(&fresh))?;
    }
    if let Some(id) = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        && let Ok(mut s) = session.lock()
    {
        *s = Some(id.to_string());
    }
    match resp.status().as_u16() {
        200..=299 => Ok(resp),
        401 | 403 => Err(SIGN_IN.into()),
        code => {
            let body = resp.into_body().read_to_string().unwrap_or_default();
            Err(format!(
                "{url} returned {code}: {}",
                body.trim().chars().take(300).collect::<String>()
            ))
        }
    }
}

impl HttpConn {
    fn start(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(u64, Receiver<Result<Value, String>>), String> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = mpsc::channel();
        let (url, headers, session, initialized, cwd) = (
            self.url.clone(),
            self.headers.clone(),
            self.session.clone(),
            self.initialized,
            self.cwd.clone(),
        );
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        std::thread::spawn(move || {
            let result = (|| {
                let resp = http_post(&url, &url, &headers, &session, initialized, &msg)?;
                let kind = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let outcome = |m: &Value| match m.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                    None => Ok(m["result"].clone()),
                };
                if !kind.starts_with("text/event-stream") {
                    let body = resp
                        .into_body()
                        .read_to_string()
                        .map_err(|e| e.to_string())?;
                    let m: Value =
                        serde_json::from_str(&body).map_err(|e| format!("bad reply: {e}"))?;
                    return outcome(&m);
                }
                let mut answer = None;
                let reader = BufReader::new(resp.into_body().into_reader());
                crate::sse::read(reader, |data| {
                    let Ok(m) = serde_json::from_str::<Value>(data) else {
                        return Ok(true);
                    };
                    if m.get("method").is_some() {
                        if let Some(reply) = answer_server_request(&m, &cwd) {
                            http_post(&url, &url, &headers, &session, true, &reply)?;
                        }
                        return Ok(true);
                    }
                    if m["id"].as_u64() == Some(id) {
                        answer = Some(outcome(&m));
                        return Ok(false);
                    }
                    Ok(true)
                })?;
                answer
                    .unwrap_or_else(|| Err("the server closed the stream without replying".into()))
            })();
            let _ = tx.send(result);
        });
        Ok((id, rx))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        http_post(
            &self.url,
            &self.url,
            &self.headers,
            &self.session,
            self.initialized,
            &msg,
        )
        .map(|_| ())
    }
}

/// The older HTTP+SSE transport: a GET opens an event stream whose first
/// event names the endpoint to POST messages to; replies, and the server's
/// own requests, come back as events on that stream.
struct SseConn {
    url: String,
    endpoint: String,
    headers: Vec<(String, String)>,
    session: Arc<Mutex<Option<String>>>,
    pending: Pending,
    open: Arc<std::sync::atomic::AtomicBool>,
    initialized: bool,
    next_id: u64,
}

/// `endpoint` as an absolute URL: absolute already, from the root of
/// `base`'s host, or next to `base`.
fn resolve_url(base: &str, endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.to_string();
    }
    let host_end = base
        .find("://")
        .and_then(|i| base[i + 3..].find('/').map(|j| i + 3 + j))
        .unwrap_or(base.len());
    if endpoint.starts_with('/') {
        format!("{}{endpoint}", &base[..host_end])
    } else {
        let dir = base[..base
            .rfind('/')
            .filter(|&i| i >= host_end)
            .unwrap_or(base.len())]
            .to_string();
        format!("{dir}/{endpoint}")
    }
}

impl SseConn {
    fn open(cfg: &ServerConfig, url: &str, cwd: &Path) -> Result<SseConn, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let own = cfg
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"));
        let get = |token: Option<&str>| {
            let mut req = agent.get(url).header("accept", "text/event-stream");
            for (k, v) in &cfg.headers {
                req = req.header(k, v);
            }
            if let Some(t) = token {
                req = req.header("authorization", &format!("Bearer {t}"));
            }
            req.call()
                .map_err(|e| format!("could not reach {url}: {e}"))
        };
        let token = if own {
            None
        } else {
            crate::oauth::bearer(url, false)
        };
        let mut resp = get(token.as_deref())?;
        if resp.status().as_u16() == 401
            && token.is_some()
            && let Some(fresh) = crate::oauth::bearer(url, true)
        {
            resp = get(Some(&fresh))?;
        }
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(SIGN_IN.into()),
            code => return Err(format!("{url} returned {code}")),
        }
        let pending: Pending = Arc::default();
        let open = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (endpoint_tx, endpoint_rx) = mpsc::channel();
        let (p, alive, base, headers, cwd) = (
            pending.clone(),
            open.clone(),
            url.to_string(),
            cfg.headers.clone(),
            cwd.to_path_buf(),
        );
        std::thread::spawn(move || {
            let mut endpoint: Option<String> = None;
            let reader = BufReader::new(resp.into_body().into_reader());
            let session = Mutex::new(None);
            let _ = crate::sse::read(reader, |data| {
                let Ok(msg) = serde_json::from_str::<Value>(data) else {
                    // The first event is the endpoint, a URL rather than JSON.
                    if endpoint.is_none() {
                        let e = resolve_url(&base, data.trim());
                        endpoint = Some(e.clone());
                        let _ = endpoint_tx.send(e);
                    }
                    return Ok(true);
                };
                if msg.get("method").is_some() {
                    if let (Some(reply), Some(e)) = (answer_server_request(&msg, &cwd), &endpoint) {
                        let _ = http_post(e, &base, &headers, &session, true, &reply);
                    }
                    return Ok(true);
                }
                if let Some(id) = msg["id"].as_u64() {
                    let result = match msg.get("error") {
                        Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                        None => Ok(msg["result"].clone()),
                    };
                    if let Some(tx) = p.lock().ok().and_then(|mut p| p.remove(&id)) {
                        let _ = tx.send(result);
                    }
                }
                Ok(true)
            });
            alive.store(false, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut p) = p.lock() {
                for (_, tx) in p.drain() {
                    let _ = tx.send(Err("the server closed its event stream".into()));
                }
            }
        });
        let endpoint = endpoint_rx
            .recv_timeout(START_TIMEOUT)
            .map_err(|_| format!("{url} did not say where to send messages"))?;
        Ok(SseConn {
            url: url.to_string(),
            endpoint,
            headers: cfg.headers.clone(),
            session: Arc::default(),
            pending,
            open,
            initialized: false,
            next_id: 1,
        })
    }

    fn post(&self, msg: &Value) -> Result<(), String> {
        http_post(
            &self.endpoint,
            &self.url,
            &self.headers,
            &self.session,
            self.initialized,
            msg,
        )
        .map(|_| ())
    }

    fn start(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(u64, Receiver<Result<Value, String>>), String> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = mpsc::channel();
        self.pending.lock().map_err(|_| "poisoned")?.insert(id, tx);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(e) = self.post(&msg) {
            if let Ok(mut p) = self.pending.lock() {
                p.remove(&id);
            }
            return Err(e);
        }
        Ok((id, rx))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.post(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }
}

enum Connection {
    Stdio(StdioConn),
    Http(HttpConn),
    Sse(SseConn),
}

impl Connection {
    fn open(cfg: &ServerConfig, cwd: &Path) -> Result<Connection, String> {
        Ok(match &cfg.url {
            Some(url) if cfg.sse => Connection::Sse(SseConn::open(cfg, url, cwd)?),
            Some(url) => Connection::Http(HttpConn {
                url: url.clone(),
                headers: cfg.headers.clone(),
                session: Arc::default(),
                initialized: false,
                next_id: 1,
                cwd: cwd.to_path_buf(),
            }),
            None => Connection::Stdio(StdioConn::spawn(cfg, cwd)?),
        })
    }

    fn start(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(u64, Receiver<Result<Value, String>>), String> {
        match self {
            Connection::Stdio(c) => c.start(method, params),
            Connection::Http(c) => c.start(method, params),
            Connection::Sse(c) => c.start(method, params),
        }
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let (_, rx) = self.start(method, params)?;
        rx.recv_timeout(timeout)
            .map_err(|_| format!("{method} timed out after {}s", timeout.as_secs()))?
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        match self {
            Connection::Stdio(c) => c.notify(method, params),
            Connection::Http(c) => c.notify(method, params),
            Connection::Sse(c) => c.notify(method, params),
        }
    }

    /// After `initialize`: HTTP requests then carry the protocol version.
    fn initialized(&mut self) {
        match self {
            Connection::Http(c) => c.initialized = true,
            Connection::Sse(c) => c.initialized = true,
            Connection::Stdio(_) => {}
        }
    }

    fn alive(&mut self) -> bool {
        match self {
            Connection::Stdio(c) => matches!(c.child.try_wait(), Ok(None)),
            Connection::Http(_) => true,
            Connection::Sse(c) => c.open.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Stops waiting for a request the user cancelled.
    fn forget(&self, id: u64) {
        let pending = match self {
            Connection::Stdio(c) => &c.pending,
            Connection::Sse(c) => &c.pending,
            Connection::Http(_) => return,
        };
        if let Ok(mut p) = pending.lock() {
            p.remove(&id);
        }
    }
}

/// Requests a server makes of axio: ping and roots are answered, the rest
/// get "method not found".
fn answer_server_request(msg: &Value, cwd: &Path) -> Option<Value> {
    let id = msg.get("id")?;
    let reply = match msg["method"].as_str() {
        Some("ping") => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        Some("roots/list") => json!({"jsonrpc": "2.0", "id": id, "result": {"roots": [
            {"uri": format!("file://{}", cwd.display()), "name": "workspace"}
        ]}}),
        _ => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})
        }
    };
    Some(reply)
}

struct Server {
    config: ServerConfig,
    conn: Option<Connection>,
    /// Model-facing tool name to the server's own name.
    tools: Vec<(String, String)>,
    /// How to use the server, from its `initialize` reply.
    instructions: Option<String>,
}

impl Server {
    fn connect(&mut self, cwd: &Path) -> Result<Vec<Value>, String> {
        let name = self.config.name.clone();
        self.handshake(cwd)
            .map_err(|e| if e == SIGN_IN { sign_in_hint(&name) } else { e })
    }

    fn handshake(&mut self, cwd: &Path) -> Result<Vec<Value>, String> {
        let mut conn = Connection::open(&self.config, cwd)?;
        let init = conn.request(
            "initialize",
            json!({"protocolVersion": PROTOCOL, "capabilities": {"roots": {}},
                   "clientInfo": {"name": "axio", "version": env!("CARGO_PKG_VERSION")}}),
            START_TIMEOUT,
        )?;
        self.instructions = init["instructions"]
            .as_str()
            .map(str::trim)
            .filter(|i| !i.is_empty())
            .map(String::from);
        conn.initialized();
        conn.notify("notifications/initialized", json!({}))?;
        let mut tools = Vec::new();
        let mut cursor = Value::Null;
        loop {
            let params = if cursor.is_null() {
                json!({})
            } else {
                json!({"cursor": cursor})
            };
            let page = conn.request("tools/list", params, START_TIMEOUT)?;
            tools.extend(page["tools"].as_array().cloned().unwrap_or_default());
            cursor = page["nextCursor"].clone();
            if !cursor.is_string() {
                break;
            }
        }
        self.conn = Some(conn);
        Ok(tools)
    }
}

/// Every configured server, started together when a session begins.
#[derive(Default)]
pub struct Mcp {
    servers: Vec<Mutex<Server>>,
    pub specs: Vec<ToolSpec>,
    /// Servers that failed to start, with the reason.
    pub failed: Vec<String>,
    cwd: PathBuf,
}

fn tool_name(server: &str, tool: &str, taken: &[String]) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let mut name: String = format!("mcp__{}__{}", clean(server), clean(tool));
    name.truncate(NAME_LIMIT);
    let base = name.clone();
    let mut n = 2;
    while taken.contains(&name) {
        let suffix = format!("_{n}");
        name = format!(
            "{}{suffix}",
            &base[..base.len().min(NAME_LIMIT - suffix.len())]
        );
        n += 1;
    }
    name
}

impl Mcp {
    pub fn start(config: &Config, cwd: &Path) -> Mcp {
        let started: Vec<(Server, Result<Vec<Value>, String>)> = std::thread::scope(|s| {
            let handles: Vec<_> = config
                .servers
                .iter()
                .map(|c| {
                    s.spawn(move || {
                        let mut server = Server {
                            config: c.clone(),
                            conn: None,
                            tools: Vec::new(),
                            instructions: None,
                        };
                        let r = server.connect(cwd);
                        (server, r)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        let mut mcp = Mcp {
            cwd: cwd.to_path_buf(),
            ..Mcp::default()
        };
        for (mut server, result) in started {
            let tools = match result {
                Ok(t) => t,
                Err(e) => {
                    mcp.failed.push(format!("{}: {e}", server.config.name));
                    continue;
                }
            };
            for t in tools {
                let Some(original) = t["name"].as_str() else {
                    continue;
                };
                let taken: Vec<String> = mcp.specs.iter().map(|s| s.name.clone()).collect();
                let name = tool_name(&server.config.name, original, &taken);
                let mut schema = t["inputSchema"].clone();
                if !schema.is_object() {
                    schema = json!({"type": "object", "properties": {}});
                }
                mcp.specs.push(ToolSpec {
                    name: name.clone(),
                    description: t["description"].as_str().unwrap_or("").to_string(),
                    schema,
                });
                server.tools.push((name, original.to_string()));
            }
            mcp.servers.push(Mutex::new(server));
        }
        mcp
    }

    /// Servers' own instructions for the system prompt, as Claude Code
    /// passes them; with `only`, just the servers owning one of those tools.
    pub fn prompt_section(&self, only: Option<&[String]>) -> Option<String> {
        let parts: Vec<String> = self
            .servers
            .iter()
            .filter_map(|s| s.lock().ok())
            .filter(|s| only.is_none_or(|names| s.tools.iter().any(|(n, _)| names.contains(n))))
            .filter_map(|s| {
                let text = s.instructions.as_ref()?;
                Some(format!(
                    "<mcp-server-instructions server=\"{}\">\n{text}\n</mcp-server-instructions>",
                    s.config.name
                ))
            })
            .collect();
        (!parts.is_empty()).then(|| {
            format!(
                "Instructions from the MCP servers whose tools you have:\n\n{}",
                parts.join("\n\n")
            )
        })
    }

    pub fn owns(&self, tool: &str) -> bool {
        self.specs.iter().any(|s| s.name == tool)
    }

    /// Calls a tool, restarting its server once if it has exited. `stop` is
    /// polled; stopping sends `notifications/cancelled`.
    pub fn call(
        &self,
        tool: &str,
        input: &Value,
        stop: &mut dyn FnMut() -> bool,
    ) -> (Result<String, String>, Vec<Block>) {
        match self.call_raw(tool, input, stop) {
            Ok(result) => render(&result),
            Err(e) => (Err(e), Vec::new()),
        }
    }

    fn call_raw(
        &self,
        tool: &str,
        input: &Value,
        stop: &mut dyn FnMut() -> bool,
    ) -> Result<Value, String> {
        let server = self
            .servers
            .iter()
            .find(|s| {
                s.lock()
                    .is_ok_and(|s| s.tools.iter().any(|(n, _)| n == tool))
            })
            .ok_or_else(|| format!("unknown tool: {tool}"))?;
        let mut server = server.lock().map_err(|_| "server state poisoned")?;
        let original = server
            .tools
            .iter()
            .find(|(n, _)| n == tool)
            .map(|(_, o)| o.clone())
            .unwrap();
        if !server.conn.as_mut().is_some_and(Connection::alive) {
            server
                .connect(&self.cwd)
                .map_err(|e| format!("the {} server could not restart: {e}", server.config.name))?;
        }
        let timeout = server.config.timeout;
        let conn = server.conn.as_mut().unwrap();
        let (id, rx) = conn.start(
            "tools/call",
            json!({"name": original, "arguments": if input.is_object() { input.clone() } else { json!({}) }}),
        )?;
        let started = Instant::now();
        let result = loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(r) => {
                    break r.map_err(|e| {
                        if e == SIGN_IN {
                            sign_in_hint(&server.config.name)
                        } else {
                            e
                        }
                    })?;
                }
                Err(RecvTimeoutError::Disconnected) => return Err("the server exited".into()),
                Err(RecvTimeoutError::Timeout) => {
                    let reason = if stop() {
                        crate::tools::STOPPED.to_string()
                    } else if started.elapsed() >= timeout {
                        format!("[timed out after {}s]", timeout.as_secs())
                    } else {
                        continue;
                    };
                    let _ = conn.notify(
                        "notifications/cancelled",
                        json!({"requestId": id, "reason": reason}),
                    );
                    conn.forget(id);
                    return Err(reason);
                }
            }
        };
        Ok(result)
    }
}

/// Tool results as text plus the images in them. Text and text resources are
/// kept; images (also image resources) go to the model after the result;
/// anything else is named so the model knows it was there.
fn render(result: &Value) -> (Result<String, String>, Vec<Block>) {
    let mut parts = Vec::new();
    let mut images = Vec::new();
    let mut image = |data: &str, mime: &str, from: Option<&str>, parts: &mut Vec<String>| {
        let bytes = data.trim_end_matches('=').len() * 3 / 4;
        let why = if data.is_empty() {
            Some("it is empty".to_string())
        } else if !matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            Some(format!("{mime} is not a PNG, JPEG, GIF or WebP image"))
        } else if bytes > MAX_IMAGE_BYTES {
            Some(format!("too large to show ({:.1} MB)", bytes as f64 / 1e6))
        } else {
            None
        };
        if let Some(why) = why {
            parts.push(format!("[image omitted: {why}]"));
            return;
        }
        images.push(Block::Image {
            media_type: mime.into(),
            data: data.into(),
        });
        let n = images.len();
        parts.push(match from {
            Some(uri) => format!("[image {n} ({uri}): {mime}, shown below]"),
            None => format!("[image {n}: {mime}, shown below]"),
        });
    };
    for item in result["content"].as_array().into_iter().flatten() {
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        match item["type"].as_str() {
            Some("text") => parts.push(s(&item["text"])),
            Some("image") => image(&s(&item["data"]), &s(&item["mimeType"]), None, &mut parts),
            Some("resource") => {
                let r = &item["resource"];
                let uri = s(&r["uri"]);
                let mime = r["mimeType"].as_str().unwrap_or("application/octet-stream");
                if let Some(text) = r["text"].as_str() {
                    parts.push(format!("<resource uri=\"{uri}\">\n{text}\n</resource>"));
                } else if let Some(blob) = r["blob"].as_str() {
                    if mime.starts_with("image/") {
                        image(blob, mime, Some(&uri), &mut parts);
                    } else {
                        let bytes = blob.trim_end_matches('=').len() * 3 / 4;
                        parts.push(format!(
                            "[resource {uri}: {mime}, {bytes} bytes, not shown]"
                        ));
                    }
                } else {
                    parts.push(format!("[resource {uri}]"));
                }
            }
            Some("resource_link") => parts.push(format!(
                "[resource link: {} <{}>]",
                item["name"].as_str().unwrap_or("unnamed"),
                s(&item["uri"])
            )),
            Some(other) => parts.push(format!("[{other} content omitted]")),
            None => {}
        }
    }
    if parts.is_empty() && !result["structuredContent"].is_null() {
        parts.push(result["structuredContent"].to_string());
    }
    let text = parts.join("\n");
    if result["isError"] == true {
        (Err(text), Vec::new())
    } else {
        (Ok(text), images)
    }
}

/// A tiny MCP server used by the tests: the test binary re-runs itself with
/// `AXIO_MCP_STUB` set and serves on stdin/stdout.
#[cfg(test)]
pub mod stub {
    use super::*;

    pub fn serve() {
        let stdin = std::io::stdin();
        let mut out = std::io::stdout();
        for line in stdin.lock().lines() {
            let Ok(msg) = serde_json::from_str::<Value>(&line.unwrap()) else {
                continue;
            };
            let Some(id) = msg.get("id").cloned() else {
                continue;
            };
            let result = match msg["method"].as_str().unwrap_or("") {
                "initialize" => {
                    json!({"protocolVersion": PROTOCOL, "capabilities": {"tools": {}}, "serverInfo": {"name": "stub"},
                           "instructions": "Call echo before anything else."})
                }
                "tools/list" if msg["params"]["cursor"].is_null() => json!({"tools": [
                    {"name": "echo", "description": "Echo the text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}
                ], "nextCursor": "2"}),
                "tools/list" => json!({"tools": [
                    {"name": "sleep", "description": "Never answers", "inputSchema": {"type": "object"}},
                    {"name": "fail", "description": "Errors", "inputSchema": {"type": "object"}},
                    {"name": "crash", "description": "Exits", "inputSchema": {"type": "object"}}
                ]}),
                "tools/call" => match msg["params"]["name"].as_str() {
                    Some("echo") => json!({"content": [
                        {"type": "text", "text": format!("echo: {}", msg["params"]["arguments"]["text"].as_str().unwrap_or(""))},
                        {"type": "image", "data": "", "mimeType": "image/png"}
                    ]}),
                    Some("fail") => {
                        json!({"isError": true, "content": [{"type": "text", "text": "it broke"}]})
                    }
                    Some("crash") => std::process::exit(0),
                    _ => continue,
                },
                _ => continue,
            };
            println!("not json: servers sometimes log here");
            writeln!(
                out,
                "{}",
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            )
            .unwrap();
            out.flush().unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Mcp {
        /// The result without its images.
        fn call_text(
            &self,
            tool: &str,
            input: &Value,
            stop: &mut dyn FnMut() -> bool,
        ) -> Result<String, String> {
            self.call(tool, input, stop).0
        }
    }

    #[test]
    fn results_keep_images_resources_and_links() {
        use crate::message::Block;
        let png = crate::crypto::base64(b"\x89PNG fake");
        let (text, images) = render(&json!({"content": [
            {"type": "text", "text": "hello"},
            {"type": "image", "data": png, "mimeType": "image/png"},
            {"type": "image", "data": "", "mimeType": "image/png"},
            {"type": "image", "data": "AAAA", "mimeType": "image/tiff"},
            {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "inside", "mimeType": "text/plain"}},
            {"type": "resource", "resource": {"uri": "file:///b.png", "blob": png, "mimeType": "image/png"}},
            {"type": "resource", "resource": {"uri": "file:///c.bin", "blob": "AAAA", "mimeType": "application/octet-stream"}},
            {"type": "resource_link", "uri": "file:///d.md", "name": "d"},
            {"type": "audio", "data": "AAAA", "mimeType": "audio/wav"}
        ]}));
        assert_eq!(
            text.unwrap().lines().collect::<Vec<_>>(),
            [
                "hello",
                "[image 1: image/png, shown below]",
                "[image omitted: it is empty]",
                "[image omitted: image/tiff is not a PNG, JPEG, GIF or WebP image]",
                "<resource uri=\"file:///a.txt\">",
                "inside",
                "</resource>",
                "[image 2 (file:///b.png): image/png, shown below]",
                "[resource file:///c.bin: application/octet-stream, 3 bytes, not shown]",
                "[resource link: d <file:///d.md>]",
                "[audio content omitted]",
            ]
        );
        assert_eq!(images.len(), 2);
        assert!(
            matches!(&images[0], Block::Image { media_type, data } if media_type == "image/png" && *data == png)
        );
        let huge = "A".repeat(6_000_000);
        let (text, images) =
            render(&json!({"content": [{"type": "image", "data": huge, "mimeType": "image/png"}]}));
        assert!(images.is_empty());
        assert!(text.unwrap().contains("too large"));
    }

    #[test]
    fn stub_server() {
        if std::env::var_os("AXIO_MCP_STUB").is_some() {
            stub::serve();
        }
    }

    fn config() -> Config {
        let exe = std::env::current_exe().unwrap();
        let mut c = Config::default();
        c.add(
            &json!({"mcpServers": {
                "stub server": {"command": exe.display().to_string(),
                                "args": ["--exact", "mcp::tests::stub_server", "--nocapture", "--quiet", "--test-threads=1"],
                                "env": {"AXIO_MCP_STUB": "1"}, "timeout": 2000},
                "remote": {"type": "http", "url": "http://127.0.0.1:9/mcp"},
                "old": {"type": "sse", "url": "http://127.0.0.1:9/sse"},
                "off": {"command": "x", "disabled": true},
                "missing": {"command": "/nonexistent/axio-mcp-server"}
            }}),
            None,
            Path::new("mcp.json"),
        );
        c
    }

    #[test]
    fn loads_config_and_skips_what_it_cannot_run() {
        let c = config();
        assert_eq!(
            c.servers
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["missing", "old", "remote", "stub server"]
        );
        assert_eq!(c.servers[1].url.as_deref(), Some("http://127.0.0.1:9/sse"));
        assert!(c.servers[1].sse && !c.servers[2].sse);
        assert_eq!(c.servers[2].url.as_deref(), Some("http://127.0.0.1:9/mcp"));
        assert!(c.skipped.is_empty(), "{:?}", c.skipped);
    }

    /// An MCP server on the older HTTP+SSE transport: GET opens the event
    /// stream, whose first event names the endpoint for POSTs; replies come
    /// back on the stream.
    fn sse_server() -> String {
        use std::net::{TcpListener, TcpStream};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stream: Arc<Mutex<Option<TcpStream>>> = Arc::default();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let stream = stream.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(conn.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                        if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap();
                        }
                    }
                    let mut conn = conn;
                    if first.starts_with("GET /sse ") {
                        write!(conn, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\nevent: endpoint\ndata: /messages?sessionId=abc\n\n").unwrap();
                        conn.flush().unwrap();
                        *stream.lock().unwrap() = Some(conn);
                        return;
                    }
                    assert!(
                        first.starts_with("POST /messages?sessionId=abc "),
                        "{first}"
                    );
                    let mut body = vec![0; length];
                    std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                    write!(
                        conn,
                        "HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .unwrap();
                    let msg: Value = serde_json::from_slice(&body).unwrap();
                    let Some(id) = msg.get("id") else { return };
                    let result = match msg["method"].as_str().unwrap_or("") {
                        "initialize" => {
                            json!({"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "old"}})
                        }
                        "tools/list" => {
                            json!({"tools": [{"name": "echo", "description": "Echo", "inputSchema": {"type": "object"}}]})
                        }
                        "tools/call" => {
                            json!({"content": [{"type": "text", "text": format!("echo: {}", msg["params"]["arguments"]["text"].as_str().unwrap_or(""))}]})
                        }
                        _ => return,
                    };
                    let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
                    let mut s = stream.lock().unwrap();
                    let s = s.as_mut().expect("the stream is open");
                    write!(s, "event: message\ndata: {reply}\n\n").unwrap();
                    s.flush().unwrap();
                });
            }
        });
        format!("{base}/sse")
    }

    #[test]
    fn sse_endpoints_resolve_against_the_stream_url() {
        let base = "https://h.example:8443/v1/sse?x=1";
        assert_eq!(
            resolve_url(base, "/messages?s=1"),
            "https://h.example:8443/messages?s=1"
        );
        assert_eq!(
            resolve_url(base, "messages?s=1"),
            "https://h.example:8443/v1/messages?s=1"
        );
        assert_eq!(resolve_url(base, "http://o/m"), "http://o/m");
        assert_eq!(resolve_url("http://h", "/m"), "http://h/m");
    }

    #[test]
    fn speaks_the_older_sse_transport() {
        let mut c = Config::default();
        c.add(
            &json!({"mcpServers": {"old": {"type": "sse", "url": sse_server()}}}),
            None,
            Path::new("mcp.json"),
        );
        let mcp = Mcp::start(&c, &std::env::temp_dir());
        assert!(mcp.failed.is_empty(), "{:?}", mcp.failed);
        let names: Vec<&str> = mcp.specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["mcp__old__echo"]);
        assert_eq!(
            mcp.call_text("mcp__old__echo", &json!({"text": "hi"}), &mut || false),
            Ok("echo: hi".into())
        );
    }

    #[test]
    fn server_instructions_reach_the_prompt() {
        let mcp = Mcp::start(&config(), &std::env::temp_dir());
        let all = mcp
            .prompt_section(None)
            .expect("the stub sends instructions");
        assert!(
            all.contains("<mcp-server-instructions server=\"stub server\">"),
            "{all}"
        );
        assert!(all.contains("Call echo before anything else."));
        let granted = ["mcp__stub_server__echo".to_string()];
        assert!(mcp.prompt_section(Some(&granted)).is_some());
        assert_eq!(
            mcp.prompt_section(Some(&["mcp__other__x".to_string()])),
            None
        );
    }

    #[test]
    fn lists_and_calls_tools_across_pages() {
        let mcp = Mcp::start(&config(), &std::env::temp_dir());
        assert_eq!(mcp.failed.len(), 3);
        assert!(mcp.failed[0].starts_with("missing: could not start"));
        assert!(
            mcp.failed[1].starts_with("old: could not reach"),
            "{:?}",
            mcp.failed
        );
        assert!(mcp.failed[2].starts_with("remote: could not reach"));
        let names: Vec<&str> = mcp.specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "mcp__stub_server__echo",
                "mcp__stub_server__sleep",
                "mcp__stub_server__fail",
                "mcp__stub_server__crash"
            ]
        );
        let never = &mut || false;
        assert_eq!(
            mcp.call_text("mcp__stub_server__echo", &json!({"text": "hi"}), never),
            Ok("echo: hi\n[image omitted: it is empty]".into())
        );
        assert_eq!(
            mcp.call_text("mcp__stub_server__fail", &json!({}), never),
            Err("it broke".into())
        );
    }

    #[test]
    fn stops_times_out_and_restarts() {
        let mcp = Mcp::start(&config(), &std::env::temp_dir());
        let mut polls = 0;
        let started = Instant::now();
        let stopped = mcp.call_text("mcp__stub_server__sleep", &json!({}), &mut || {
            polls += 1;
            polls > 5
        });
        assert_eq!(stopped, Err(crate::tools::STOPPED.into()));
        assert!(started.elapsed() < Duration::from_secs(1));
        let timed_out = mcp.call_text("mcp__stub_server__sleep", &json!({}), &mut || false);
        assert_eq!(timed_out, Err("[timed out after 2s]".into()));
        assert_eq!(
            mcp.call_text("mcp__stub_server__crash", &json!({}), &mut || false),
            Err("the server exited".into())
        );
        assert_eq!(
            mcp.call_text(
                "mcp__stub_server__echo",
                &json!({"text": "back"}),
                &mut || false
            ),
            Ok("echo: back\n[image omitted: it is empty]".into()),
            "the server restarts on the next call"
        );
    }

    #[test]
    fn tool_names_are_clean_unique_and_short() {
        assert_eq!(
            tool_name("my.server", "do it", &[]),
            "mcp__my_server__do_it"
        );
        let long = "x".repeat(80);
        let first = tool_name("s", &long, &[]);
        assert_eq!(first.len(), NAME_LIMIT);
        let second = tool_name("s", &long, std::slice::from_ref(&first));
        assert!(second.len() <= NAME_LIMIT && second.ends_with("_2") && second != first);
    }

    /// A minimal streamable HTTP MCP server on a random local port. It
    /// replies with JSON or an event stream, sends a ping of its own before
    /// the tools list, insists on its session id, and refuses calls without
    /// the right Authorization header.
    fn http_server(token: &'static str) -> (String, Arc<Mutex<Vec<Value>>>) {
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = log.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut headers = HashMap::new();
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    loop {
                        line.clear();
                        reader.read_line(&mut line).unwrap();
                        let Some((k, v)) = line.trim_end().split_once(": ") else {
                            break;
                        };
                        headers.insert(k.to_ascii_lowercase(), v.to_string());
                    }
                    let len: usize = headers
                        .get("content-length")
                        .map_or(0, |l| l.parse().unwrap());
                    let mut body = vec![0; len];
                    reader.read_exact(&mut body).unwrap();
                    let msg: Value = serde_json::from_slice(&body).unwrap();
                    log.lock().unwrap().push(msg.clone());
                    let mut out = stream;
                    let reply = |out: &mut std::net::TcpStream,
                                 status: &str,
                                 kind: &str,
                                 extra: &str,
                                 body: &str| {
                        write!(out, "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
                    };
                    if headers.get("authorization").map(String::as_str) != Some(token) {
                        return reply(&mut out, "401 Unauthorized", "text/plain", "", "sign in");
                    }
                    let method = msg["method"].as_str().unwrap_or("");
                    if method != "initialize"
                        && headers.get("mcp-session-id").map(String::as_str) != Some("s1")
                    {
                        return reply(
                            &mut out,
                            "400 Bad Request",
                            "text/plain",
                            "",
                            "missing session",
                        );
                    }
                    let id = msg["id"].clone();
                    let rpc = |result: Value| {
                        json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
                    };
                    match method {
                        "initialize" => reply(
                            &mut out,
                            "200 OK",
                            "application/json",
                            "mcp-session-id: s1\r\n",
                            &rpc(
                                json!({"protocolVersion": PROTOCOL, "capabilities": {"tools": {}}}),
                            ),
                        ),
                        "tools/list" => {
                            let ping = json!({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"});
                            let tools = rpc(
                                json!({"tools": [{"name": "add", "inputSchema": {"type": "object"}}]}),
                            );
                            reply(
                                &mut out,
                                "200 OK",
                                "text/event-stream",
                                "",
                                &format!("data: {ping}\n\ndata: {tools}\n\n"),
                            );
                        }
                        "tools/call" => {
                            let a = &msg["params"]["arguments"];
                            let sum = a["a"].as_i64().unwrap_or(0) + a["b"].as_i64().unwrap_or(0);
                            reply(
                                &mut out,
                                "200 OK",
                                "application/json",
                                "",
                                &rpc(
                                    json!({"content": [{"type": "text", "text": sum.to_string()}]}),
                                ),
                            );
                        }
                        _ if msg.get("id").is_none() || msg.get("result").is_some() => {
                            reply(&mut out, "202 Accepted", "text/plain", "", "")
                        }
                        _ => reply(&mut out, "404 Not Found", "text/plain", "", ""),
                    }
                });
            }
        });
        (url, seen)
    }

    #[test]
    fn speaks_streamable_http_with_sessions_and_headers() {
        let (url, seen) = http_server("Bearer t0k");
        // SAFETY: test-only; no other test reads this variable.
        unsafe { std::env::set_var("AXIO_TEST_MCP_TOKEN", "t0k") };
        let mut c = Config::default();
        c.add(
            &json!({"mcpServers": {
                "calc": {"type": "http", "url": url, "headers": {"Authorization": "Bearer ${AXIO_TEST_MCP_TOKEN}"}},
                "locked": {"url": url}
            }}),
            None,
            Path::new("mcp.json"),
        );
        let mcp = Mcp::start(&c, &std::env::temp_dir());
        assert_eq!(mcp.failed, [format!("locked: {}", sign_in_hint("locked"))]);
        assert_eq!(
            mcp.specs
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["mcp__calc__add"]
        );
        assert_eq!(
            mcp.call_text("mcp__calc__add", &json!({"a": 2, "b": 3}), &mut || false),
            Ok("5".into())
        );
        let log = seen.lock().unwrap();
        assert!(
            log.iter()
                .any(|m| m["id"] == "srv-1" && m.get("result").is_some()),
            "the server's ping was answered over a POST"
        );
        assert!(
            log.iter()
                .any(|m| m["method"] == "notifications/initialized")
        );
    }
}
