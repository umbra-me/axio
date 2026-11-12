//! `axio agent`: one sub-agent's process. A JSON task spec arrives on stdin;
//! progress and the result leave as JSON lines on stdout. The child runs with
//! no hooks, plugins, MCP servers or imports, and only the tools it is given.

use crate::agent::{Agent, Ui};
use crate::provider::{Delta, Model, Stop, Usage};
use crate::tools::Modules;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub const RESULT_LIMIT: usize = 8000;
pub const MAX_TURNS: usize = 50;

#[derive(Deserialize, serde::Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spec {
    /// The brief: everything the sub-agent knows about its work.
    pub task: String,
    pub tools: Vec<String>,
    /// The role's instructions.
    #[serde(default)]
    pub instructions: String,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub context: Sees,
    pub max_turns: Option<usize>,
    /// Characters of the final answer returned; the session keeps it all.
    pub result_limit: Option<usize>,
    pub session_dir: Option<PathBuf>,
    /// Limits on the child's commands, tighter than none.
    pub sandbox: Option<Confine>,
    /// For `context: fork`, a transcript of the main agent's conversation.
    pub history: Option<String>,
    /// MCP tools the main agent runs for this child, which asks for each
    /// call on stdout and reads the result on stdin.
    #[serde(default)]
    pub mcp: Vec<crate::tools::ToolSpec>,
}

/// A sandbox for a sub-agent's Bash, Write and Edit.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confine {
    /// Writes only in temporary folders, no network.
    ReadOnly,
    /// Writes in the project and temporary folders, no network.
    Project,
}

/// What the sub-agent sees besides its brief.
#[derive(Clone, Copy, Deserialize, serde::Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Sees {
    #[default]
    Brief,
    /// The brief plus AGENTS.md / CLAUDE.md.
    Rules,
    /// The rules plus a copy of the main agent's conversation.
    Fork,
}

/// The modules a tool list needs. Background tasks would outlive the child,
/// so it never gets them.
fn modules_for(tools: &[String]) -> Modules {
    let has = |n: &str| tools.iter().any(|t| t == n);
    Modules {
        search: has("Grep") || has("Glob"),
        plan: has("TodoWrite"),
        web: has("WebFetch"),
        ..Modules::default()
    }
}

/// Set when the main agent closes an attached child's input: stop the turn.
static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The sub-agent for `spec`, with its own session in `session_dir`, by
/// default an `agents` folder that `-c` and `-r` do not list. `host` runs
/// its own sub-agents when its tools include Agent.
pub fn agent(
    spec: &Spec,
    home: &Path,
    cwd: PathBuf,
    host: Option<crate::subagent::Host>,
) -> Result<Agent, String> {
    let model = spec
        .model
        .as_deref()
        .unwrap_or(crate::provider::DEFAULT_MODEL);
    let system =
        crate::context::sub_agent_prompt(&cwd, &spec.instructions, spec.context != Sees::Brief);
    if spec.context == Sees::Fork && spec.history.is_none() {
        return Err("context fork needs the main agent's conversation".into());
    }
    let mut a = Agent::new(Model::parse(model), cwd.clone(), system);
    a.effort = spec.effort.clone();
    a.max_turns = Some(spec.max_turns.unwrap_or(MAX_TURNS));
    a.set_tools(modules_for(&spec.tools), crate::mcp::Mcp::default());
    a.add_tools(spec.mcp.clone());
    if let Some(h) = host {
        a.set_subagents(h);
    }
    a.keep_tools(&spec.tools)?;
    let dir = match &spec.session_dir {
        Some(d) => d.clone(),
        None => crate::session::dir_for(home, &cwd).join("agents"),
    };
    a.session = Some(crate::session::Session::create(&dir, &cwd)?);
    Ok(a)
}

/// MCP results from the main agent: (call id, result).
type Replies = std::sync::mpsc::Receiver<(u64, Result<String, String>)>;
static REPLIES: std::sync::Mutex<Option<Replies>> = std::sync::Mutex::new(None);

/// Writes progress as JSON lines. Once the reader has gone, the turn stops.
struct Lines<'a> {
    out: &'a mut dyn Write,
    cwd: PathBuf,
    usage: Usage,
    gone: bool,
    /// MCP tools the main agent runs for this child.
    remote: Vec<String>,
    next_call: u64,
}

impl Lines<'_> {
    fn emit(&mut self, v: Value) {
        if writeln!(self.out, "{v}")
            .and_then(|_| self.out.flush())
            .is_err()
        {
            self.gone = true;
        }
    }
}

impl Ui for Lines<'_> {
    fn delta(&mut self, _: Delta) {}
    fn tool_start(&mut self, name: &str, input: &Value) {
        let (_, detail) = crate::ui::tool_title(name, input, &self.cwd);
        self.emit(json!({"type": "tool", "name": name, "detail": detail}));
    }
    fn tool_end(&mut self, result: &Result<String, String>) {
        self.emit(json!({"type": "tool_end", "ok": result.is_ok()}));
    }
    fn stopped(&mut self, _: &Stop) {}
    fn usage(&mut self, u: Usage) {
        self.usage.input += u.input;
        self.usage.output += u.output;
        self.usage.cache_read += u.cache_read;
        self.usage.cache_write += u.cache_write;
    }
    fn interrupted(&mut self) -> bool {
        self.gone || CANCEL.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn notice(&mut self, text: &str) {
        self.emit(json!({"type": "notice", "text": text}));
    }
    fn remote_tool(&mut self, name: &str, input: &Value) -> Option<Result<String, String>> {
        if !self.remote.iter().any(|r| r == name) {
            return None;
        }
        self.next_call += 1;
        let id = self.next_call;
        self.emit(json!({"type": "mcp_call", "id": id, "name": name, "input": input}));
        let guard = REPLIES.lock().ok()?;
        let Some(rx) = guard.as_ref() else {
            return Some(Err("MCP tools need an attached sub-agent".into()));
        };
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok((got, r)) if got == id => return Some(r),
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !self.interrupted() => continue,
                Err(_) => return Some(Err(crate::tools::STOPPED.into())),
            }
        }
    }
}

/// Runs the task and writes its lines. Returns whether it succeeded.
pub fn drive(mut agent: Agent, spec: &Spec, out: &mut dyn Write) -> bool {
    let session = agent.session.as_ref().map(|s| s.path.display().to_string());
    let mut lines = Lines {
        out,
        cwd: agent.cwd.clone(),
        usage: Usage::default(),
        gone: false,
        remote: spec.mcp.iter().map(|t| t.name.clone()).collect(),
        next_call: 0,
    };
    let tools = agent.tool_names();
    let model = format!("{}/{}", agent.model.provider, agent.model.id);
    lines.emit(json!({"type": "start", "model": model, "tools": tools, "session": session}));
    let task = match &spec.history {
        Some(h) => format!(
            "<conversation-so-far>\n{h}</conversation-so-far>\n\nThe conversation above is the main agent's, for context. Your task:\n\n{}",
            spec.task
        ),
        None => spec.task.clone(),
    };
    let result = agent.prompt(&task, &mut lines);
    let u = lines.usage;
    let usage = json!({"input": u.input, "output": u.output,
                       "cacheRead": u.cache_read, "cacheWrite": u.cache_write});
    match result {
        Ok(()) => {
            let answer = agent
                .messages
                .last()
                .filter(|m| matches!(m, crate::message::Message::Assistant { .. }))
                .map(|m| m.text())
                .unwrap_or_default();
            let limit = spec.result_limit.unwrap_or(RESULT_LIMIT);
            let truncated = answer.chars().count() > limit;
            let text: String = answer.chars().take(limit).collect();
            lines.emit(
                json!({"type": "result", "text": text, "truncated": truncated,
                              "session": session, "usage": usage}),
            );
            true
        }
        Err(e) => {
            lines.emit(json!({"type": "error", "message": e, "session": session, "usage": usage}));
            false
        }
    }
}

/// `axio agent [--attached]`: reads the spec from stdin and runs it. An
/// attached child reads one line and then stops its turn when its input
/// closes, so the main agent can stop it, and it stops if the main agent dies.
pub fn main(args: &[String]) -> Result<bool, String> {
    let attached = args.iter().any(|a| a == "--attached");
    let mut text = String::new();
    let mut stdin = std::io::stdin().lock();
    if attached {
        std::io::BufRead::read_line(&mut stdin, &mut text).map_err(|e| e.to_string())?;
        drop(stdin);
        let (tx, rx) = std::sync::mpsc::channel();
        if let Ok(mut r) = REPLIES.lock() {
            *r = Some(rx);
        }
        std::thread::spawn(move || {
            // MCP results arrive as lines; the end of input means stop.
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if v["type"] == "mcp_result"
                    && let Some(id) = v["id"].as_u64()
                {
                    let text = v["text"].as_str().unwrap_or("").to_string();
                    let _ = tx.send((id, if v["ok"] == true { Ok(text) } else { Err(text) }));
                }
            }
            CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    } else {
        std::io::Read::read_to_string(&mut stdin, &mut text).map_err(|e| e.to_string())?;
    }
    let spec: Spec = serde_json::from_str(&text).map_err(|e| format!("task spec: {e}"))?;
    let cwd = match &spec.cwd {
        Some(c) => c.clone(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let home = crate::context::home();
    // Keys and base URLs saved with `axio auth` or /login.
    crate::auth::set_home(&home);
    if let Some(c) = spec.sandbox {
        let root = crate::settings::project_root(&cwd);
        crate::sandbox::activate(match c {
            Confine::ReadOnly => crate::sandbox::Sandbox::read_only(),
            Confine::Project => crate::sandbox::Sandbox::for_project(&root, false),
        })?;
    }
    let host = if spec.tools.iter().any(|t| t == "Agent") {
        Some(nested_host(&home, &cwd)?)
    } else {
        None
    };
    let agent = agent(&spec, &home, cwd, host)?;
    Ok(drive(agent, &spec, &mut std::io::stdout().lock()))
}

/// Sub-agents of a sub-agent: the same roles and settings, read without
/// asking about trust (only a project trusted earlier counts).
fn nested_host(home: &Path, cwd: &Path) -> Result<crate::subagent::Host, String> {
    let root = crate::settings::project_root(cwd);
    let trusted =
        crate::settings::Trust::load(home).decision(&root) == crate::settings::Decision::Trusted;
    let project = trusted.then_some(root.as_path());
    let settings = crate::settings::Settings::load(home, project)?;
    let plugins: Vec<PathBuf> = crate::plugins::list(&home.join("plugins"), None)
        .into_iter()
        .map(|p| p.root)
        .collect();
    let (roles, _) = crate::roles::load(&crate::roles::dirs(home, project, &plugins));
    crate::subagent::Host::new(roles, settings.subagents, crate::subagent::depth())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Block, Message};
    use crate::provider::Reply;

    fn spec(text: &str) -> Result<Spec, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axio-child-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lines(out: &[u8]) -> Vec<Value> {
        String::from_utf8_lossy(out)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn reads_a_spec_and_refuses_typos() {
        let s = spec(r#"{"task": "t", "tools": ["Read"]}"#).unwrap();
        assert_eq!(s.context, Sees::Brief);
        assert_eq!(s.max_turns, None);
        let s = spec(r#"{"task": "t", "tools": [], "context": "rules", "maxTurns": 3}"#).unwrap();
        assert_eq!((s.context, s.max_turns), (Sees::Rules, Some(3)));
        assert!(spec(r#"{"task": "t", "tools": [], "maxTurn": 3}"#).is_err());
        let fork = spec(r#"{"task": "t", "tools": [], "context": "fork"}"#).unwrap();
        let err = agent(&fork, &temp("fork"), temp("fork"), None)
            .err()
            .unwrap();
        assert!(err.contains("fork"), "{err}");
    }

    #[test]
    fn tools_pick_their_modules_but_never_background() {
        let m = modules_for(&["Grep".into(), "Bash".into()]);
        assert!(m.search && !m.web && !m.plan && !m.background);
        assert!(modules_for(&["WebFetch".into()]).web);
        assert!(modules_for(&["TodoWrite".into()]).plan);
    }

    #[test]
    fn runs_a_task_and_returns_a_capped_answer() {
        let dir = temp("run");
        std::fs::write(dir.join("AGENTS.md"), "PROJECT RULE").unwrap();
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        let s = spec(&format!(
            r#"{{"task": "read notes", "tools": ["Read"], "instructions": "ROLE TEXT",
                "resultLimit": 10, "model": "anthropic/claude-test", "sessionDir": {:?}}}"#,
            dir.join("sessions").display().to_string()
        ))
        .unwrap();
        let mut a = agent(&s, &dir, dir.clone(), None).unwrap();
        a.set_complete(|req, _| {
            assert!(req.system.contains("ROLE TEXT"));
            assert!(
                !req.system.contains("PROJECT RULE"),
                "brief leaves out AGENTS.md"
            );
            let content = match req.messages.len() {
                1 => vec![Block::ToolCall {
                    id: "r".into(),
                    name: "Read".into(),
                    input: json!({"file_path": "notes.txt"}),
                }],
                _ => vec![Block::Text {
                    text: "the notes say hello, and more".into(),
                }],
            };
            Ok(Reply {
                stop: if req.messages.len() == 1 {
                    Stop::ToolUse
                } else {
                    Stop::EndTurn
                },
                content,
                usage: Usage {
                    input: 5,
                    output: 2,
                    ..Usage::default()
                },
            })
        });
        let mut out = Vec::new();
        assert!(drive(a, &s, &mut out));
        let l = lines(&out);
        let types: Vec<&str> = l.iter().map(|v| v["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["start", "tool", "tool_end", "result"]);
        assert_eq!(l[0]["tools"], json!(["Read"]));
        assert_eq!(l[0]["model"], "anthropic/claude-test");
        assert_eq!(
            (&l[1]["name"], &l[1]["detail"]),
            (&json!("Read"), &json!("notes.txt"))
        );
        assert_eq!(l[2]["ok"], true);
        assert_eq!(l[3]["text"], "the notes ");
        assert_eq!(l[3]["truncated"], true);
        assert_eq!(l[3]["usage"]["input"], 10);
        let session = PathBuf::from(l[3]["session"].as_str().unwrap());
        assert!(session.starts_with(dir.join("sessions")));
        let saved = crate::session::load(&session).unwrap();
        assert!(
            saved.last().unwrap().text().contains("and more"),
            "the session keeps it all"
        );
    }

    #[test]
    fn rules_adds_project_instructions() {
        let dir = temp("rules");
        std::fs::write(dir.join("AGENTS.md"), "PROJECT RULE").unwrap();
        let brief = crate::context::sub_agent_prompt(&dir, "ROLE", false);
        let rules = crate::context::sub_agent_prompt(&dir, "ROLE", true);
        assert!(brief.contains("ROLE") && !brief.contains("PROJECT RULE"));
        assert!(rules.contains("ROLE") && rules.contains("PROJECT RULE"));
        assert!(brief.contains(&dir.display().to_string()));
    }

    #[test]
    fn a_failed_task_ends_with_an_error_line() {
        let dir = temp("fail");
        let s = spec(r#"{"task": "t", "tools": ["Read"]}"#).unwrap();
        let mut a = agent(&s, &dir, dir.clone(), None).unwrap();
        a.set_complete(|_, _| Err("boom".into()));
        a.max_retries = 0;
        let mut out = Vec::new();
        assert!(!drive(a, &s, &mut out));
        let l = lines(&out);
        assert_eq!(l.last().unwrap()["type"], "error");
        assert_eq!(l.last().unwrap()["message"], "boom");
        assert!(matches!(Message::user_text("x"), Message::User { .. }));
    }

    #[test]
    fn an_unknown_tool_is_refused_before_any_call() {
        let dir = temp("unknown");
        let s = spec(r#"{"task": "t", "tools": ["Read", "Teleport"]}"#).unwrap();
        let err = agent(&s, &dir, dir.clone(), None).err().unwrap();
        assert!(err.contains("Teleport"), "{err}");
    }
}
