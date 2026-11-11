//! MCP client over stdio: axio starts each server, speaks newline-delimited
//! JSON-RPC 2.0 on its stdin and stdout, and offers its tools to the model as
//! `mcp__<server>__<tool>`.

use crate::tools::ToolSpec;
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
            let kind = s["type"].as_str().unwrap_or("stdio");
            let Some(command) = s["command"].as_str().filter(|_| kind == "stdio") else {
                let why = if s.get("url").is_some() || kind != "stdio" {
                    "HTTP and SSE servers are not supported yet"
                } else {
                    "it has no command"
                };
                self.skipped
                    .push(format!("{}: {name}: {why}", source.display()));
                continue;
            };
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

struct Connection {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: u64,
}

impl Drop for Connection {
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

impl Connection {
    fn spawn(cfg: &ServerConfig, cwd: &Path) -> Result<Connection, String> {
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
                    answer_server_request(&msg, &reader_stdin, &cwd);
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
        Ok(Connection {
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

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let (_, rx) = self.start(method, params)?;
        rx.recv_timeout(timeout)
            .map_err(|_| format!("{method} timed out after {}s", timeout.as_secs()))?
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        send(
            &self.stdin,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// Requests a server makes of axio: ping and roots are answered, the rest
/// get "method not found".
fn answer_server_request(msg: &Value, stdin: &Mutex<ChildStdin>, cwd: &Path) {
    let Some(id) = msg.get("id") else { return };
    let reply = match msg["method"].as_str() {
        Some("ping") => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        Some("roots/list") => json!({"jsonrpc": "2.0", "id": id, "result": {"roots": [
            {"uri": format!("file://{}", cwd.display()), "name": "workspace"}
        ]}}),
        _ => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})
        }
    };
    let _ = send(stdin, &reply);
}

struct Server {
    config: ServerConfig,
    conn: Option<Connection>,
    /// Model-facing tool name to the server's own name.
    tools: Vec<(String, String)>,
}

impl Server {
    fn connect(&mut self, cwd: &Path) -> Result<Vec<Value>, String> {
        let mut conn = Connection::spawn(&self.config, cwd)?;
        conn.request(
            "initialize",
            json!({"protocolVersion": PROTOCOL, "capabilities": {"roots": {}},
                   "clientInfo": {"name": "axio", "version": env!("CARGO_PKG_VERSION")}}),
            START_TIMEOUT,
        )?;
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
    ) -> Result<String, String> {
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
                Ok(r) => break r?,
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
                    if let Ok(mut p) = conn.pending.lock() {
                        p.remove(&id);
                    }
                    return Err(reason);
                }
            }
        };
        render(&result)
    }
}

/// Tool results as text: text items joined, other content named.
fn render(result: &Value) -> Result<String, String> {
    let mut parts = Vec::new();
    for item in result["content"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("text") => parts.push(item["text"].as_str().unwrap_or("").to_string()),
            Some("resource") => parts.push(
                item["resource"]["text"]
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| format!("[resource {}]", item["resource"]["uri"])),
            ),
            Some(other) => parts.push(format!("[{other} content omitted]")),
            None => {}
        }
    }
    if parts.is_empty() && !result["structuredContent"].is_null() {
        parts.push(result["structuredContent"].to_string());
    }
    let text = parts.join("\n");
    if result["isError"] == true {
        Err(text)
    } else {
        Ok(text)
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
                    json!({"protocolVersion": PROTOCOL, "capabilities": {"tools": {}}, "serverInfo": {"name": "stub"}})
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
                "remote": {"type": "http", "url": "https://example.invalid/mcp"},
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
            ["missing", "stub server"]
        );
        assert_eq!(
            c.skipped,
            ["mcp.json: remote: HTTP and SSE servers are not supported yet"]
        );
    }

    #[test]
    fn lists_and_calls_tools_across_pages() {
        let mcp = Mcp::start(&config(), &std::env::temp_dir());
        assert_eq!(mcp.failed.len(), 1);
        assert!(mcp.failed[0].starts_with("missing: could not start"));
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
            mcp.call("mcp__stub_server__echo", &json!({"text": "hi"}), never),
            Ok("echo: hi\n[image content omitted]".into())
        );
        assert_eq!(
            mcp.call("mcp__stub_server__fail", &json!({}), never),
            Err("it broke".into())
        );
    }

    #[test]
    fn stops_times_out_and_restarts() {
        let mcp = Mcp::start(&config(), &std::env::temp_dir());
        let mut polls = 0;
        let started = Instant::now();
        let stopped = mcp.call("mcp__stub_server__sleep", &json!({}), &mut || {
            polls += 1;
            polls > 5
        });
        assert_eq!(stopped, Err(crate::tools::STOPPED.into()));
        assert!(started.elapsed() < Duration::from_secs(1));
        let timed_out = mcp.call("mcp__stub_server__sleep", &json!({}), &mut || false);
        assert_eq!(timed_out, Err("[timed out after 2s]".into()));
        assert_eq!(
            mcp.call("mcp__stub_server__crash", &json!({}), &mut || false),
            Err("the server exited".into())
        );
        assert_eq!(
            mcp.call(
                "mcp__stub_server__echo",
                &json!({"text": "back"}),
                &mut || false
            ),
            Ok("echo: back\n[image content omitted]".into()),
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
}
