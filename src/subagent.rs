//! The `Agent` tool: the main agent hands a task to a sub-agent, which runs
//! as its own `axio agent --attached` process. Several calls in one reply run
//! at once, up to `subagents.parallel`.

use crate::agent::Ui;
use crate::child::{Sees, Spec};
use crate::roles::{Call, Parent, Role};
use crate::settings::Subagents;
use crate::tools::ToolSpec;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub const PARALLEL: usize = 4;

/// The depth of this process: 0 for the main agent, 1 for its sub-agents.
pub fn depth() -> u32 {
    std::env::var("AXIO_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0)
}

pub struct Host {
    pub roles: Vec<Role>,
    pub settings: Subagents,
    pub depth: u32,
    exe: PathBuf,
}

/// A sub-agent call checked and ready to start.
#[derive(Clone)]
pub struct Planned {
    pub role: String,
    pub label: String,
    pub spec: Spec,
    pub notes: Vec<String>,
}

struct Running {
    index: usize,
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Value>,
    stderr: std::thread::JoinHandle<String>,
    started: Instant,
    tools: usize,
    doing: String,
    end: Option<Value>,
}

impl Host {
    pub fn new(roles: Vec<Role>, settings: Subagents, depth: u32) -> Result<Host, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find axio itself: {e}"))?;
        Ok(Host {
            roles,
            settings,
            depth,
            exe,
        })
    }

    /// Whether this process's sub-agents may start sub-agents of their own.
    pub fn children_may_nest(&self) -> bool {
        self.depth < self.settings.nesting.unwrap_or(0)
    }

    pub fn tool_spec(&self) -> ToolSpec {
        let names: Vec<&str> = self.roles.iter().map(|r| r.name.as_str()).collect();
        let list: String = self
            .roles
            .iter()
            .map(|r| format!("\n- {}: {}", r.name, r.description))
            .collect();
        ToolSpec {
            name: "Agent".into(),
            description: format!(
                "Hand a self-contained task to a sub-agent and get back its final answer. The sub-agent starts fresh: it sees only your prompt (plus the project's AGENTS.md for roles that use rules), so write a prompt that stands on its own, with the goal, the files involved and what to report. Use sub-agents for searches, research or reviews whose details you do not need in your own context, and for independent pieces of work. Several Agent calls in one reply run at the same time. Roles:{list}"
            ),
            schema: json!({
                "type": "object",
                "properties": {
                    "description": {"type": "string", "description": "A 3-5 word label for the task, shown to the user"},
                    "prompt": {"type": "string", "description": "The full brief for the sub-agent"},
                    "subagent_type": {"type": "string", "enum": names, "description": "The role to run"},
                    "model": {"type": "string", "description": "Only for roles whose model is `choose`: provider/model, or fast"},
                    "effort": {"type": "string", "description": "Only for roles whose effort is `choose`"},
                    "context": {"type": "string", "enum": ["brief", "rules", "fork"], "description": "What the sub-agent sees besides the prompt; fork adds a transcript of this conversation. Leave out to use the role's default"}
                },
                "required": ["description", "prompt", "subagent_type"],
                "additionalProperties": false
            }),
        }
    }

    /// Checks one `Agent` call and builds its task spec.
    pub fn prepare(&self, input: &Value, parent: &Parent) -> Result<Planned, String> {
        let s = |k: &str| input[k].as_str().map(String::from);
        let role_name = s("subagent_type").ok_or("subagent_type is required")?;
        let role = self
            .roles
            .iter()
            .find(|r| r.name == role_name)
            .ok_or_else(|| {
                let names: Vec<&str> = self.roles.iter().map(|r| r.name.as_str()).collect();
                format!("no role named {role_name}; roles are {}", names.join(", "))
            })?;
        let task = s("prompt")
            .filter(|p| !p.trim().is_empty())
            .ok_or("prompt is required")?;
        let context = match s("context").as_deref() {
            None => None,
            Some("brief") => Some(Sees::Brief),
            Some("rules") => Some(Sees::Rules),
            Some("fork") => Some(Sees::Fork),
            Some(c) => return Err(format!("context must be brief, rules or fork, not {c}")),
        };
        let call = Call {
            task,
            model: s("model"),
            effort: s("effort"),
            context,
        };
        let (spec, notes) = crate::roles::spec(role, &call, parent, &self.settings)?;
        let label = match s("description").filter(|d| !d.trim().is_empty()) {
            Some(d) => format!("{} · {}", role.name, d.trim()),
            None => role.name.clone(),
        };
        Ok(Planned {
            role: role.name.clone(),
            label,
            spec,
            notes,
        })
    }

    fn spawn(&self, index: usize, plan: &Planned) -> Result<Running, String> {
        let mut cmd = Command::new(&self.exe);
        cmd.args(["agent", "--attached"])
            .env("AXIO_DEPTH", (self.depth + 1).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start a sub-agent: {e}"))?;
        let mut stdin = child.stdin.take();
        let line = serde_json::to_string(&plan.spec).map_err(|e| e.to_string())?;
        if let Some(s) = &mut stdin {
            writeln!(s, "{line}").map_err(|e| format!("could not brief the sub-agent: {e}"))?;
            let _ = s.flush();
        }
        let out = child.stdout.take().expect("piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && tx.send(v).is_err()
                {
                    break;
                }
            }
        });
        let mut err = child.stderr.take().expect("piped");
        let stderr = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        Ok(Running {
            index,
            label: plan.label.clone(),
            child,
            stdin,
            lines: rx,
            stderr,
            started: Instant::now(),
            tools: 0,
            doing: String::new(),
            end: None,
        })
    }

    /// Runs the planned calls, at most `parallel` at once, and returns their
    /// results in order. Esc stops them all.
    /// `mcp` runs the MCP calls children ask for, each on its own thread.
    pub fn run_all(
        &self,
        plans: &[&Planned],
        mcp: std::sync::Arc<crate::mcp::Mcp>,
        ui: &mut dyn Ui,
    ) -> Vec<Finished> {
        let (mcp_tx, mcp_rx) = mpsc::channel::<(usize, Value, Result<String, String>)>();
        let limit = self.settings.parallel.unwrap_or(PARALLEL).max(1);
        let mut results: Vec<Option<Finished>> = (0..plans.len()).map(|_| None).collect();
        let failed = |e: String| {
            Some(Finished {
                result: Err(e),
                session: None,
            })
        };
        let mut queue: std::collections::VecDeque<usize> = (0..plans.len()).collect();
        let mut running: Vec<Running> = Vec::new();
        loop {
            while running.len() < limit
                && let Some(i) = queue.pop_front()
            {
                match self.spawn(i, plans[i]) {
                    Ok(r) => running.push(r),
                    Err(e) => {
                        ui.agent_done(&plans[i].label, &Err(e.clone()), "");
                        results[i] = failed(e);
                    }
                }
            }
            if running.is_empty() {
                break;
            }
            let mut finished = Vec::new();
            for (n, r) in running.iter_mut().enumerate() {
                loop {
                    match r.lines.try_recv() {
                        Ok(v) => match v["type"].as_str() {
                            Some("tool") => {
                                r.tools += 1;
                                r.doing = format!(
                                    "{} {}",
                                    v["name"].as_str().unwrap_or(""),
                                    v["detail"].as_str().unwrap_or("")
                                )
                                .trim()
                                .to_string();
                            }
                            Some("result" | "error") => r.end = Some(v),
                            Some("mcp_call") => {
                                let name = v["name"].as_str().unwrap_or("").to_string();
                                let granted =
                                    plans[r.index].spec.mcp.iter().any(|t| t.name == name);
                                let (tx, mcp, index, id, input) = (
                                    mcp_tx.clone(),
                                    mcp.clone(),
                                    r.index,
                                    v["id"].clone(),
                                    v["input"].clone(),
                                );
                                if granted {
                                    std::thread::spawn(move || {
                                        let out = mcp.call(&name, &input, &mut || false);
                                        let _ = tx.send((index, id, out));
                                    });
                                } else {
                                    let _ = tx.send((
                                        index,
                                        id,
                                        Err(format!("this sub-agent may not use {name}")),
                                    ));
                                }
                            }
                            _ => {}
                        },
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            if matches!(r.child.try_wait(), Ok(Some(_))) {
                                finished.push(n);
                            }
                            break;
                        }
                    }
                }
            }
            while let Ok((index, id, out)) = mcp_rx.try_recv() {
                if let Some(r) = running.iter_mut().find(|r| r.index == index)
                    && let Some(stdin) = &mut r.stdin
                {
                    let (ok, text) = match out {
                        Ok(t) => (true, t),
                        Err(e) => (false, e),
                    };
                    let line = json!({"type": "mcp_result", "id": id, "ok": ok, "text": text});
                    let _ = writeln!(stdin, "{line}").and_then(|_| stdin.flush());
                }
            }
            for n in finished.into_iter().rev() {
                let r = running.remove(n);
                let i = r.index;
                let out = self.finish(r, plans[i], ui);
                results[i] = Some(out);
            }
            if !running.is_empty() {
                ui.agents_running(&progress(&running));
            }
            if ui.interrupted() {
                stop(&mut running);
                for r in running.drain(..) {
                    let e = format!("not finished: {}", crate::tools::STOPPED);
                    ui.agent_done(&r.label, &Err(e.clone()), "stopped");
                    results[r.index] = failed(e);
                }
                for i in queue.drain(..) {
                    results[i] = failed(format!("not run: {}", crate::tools::STOPPED));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        results
            .into_iter()
            .map(|r| {
                r.or_else(|| failed("the sub-agent did not run".into()))
                    .expect("set")
            })
            .collect()
    }

    fn finish(&self, r: Running, plan: &Planned, ui: &mut dyn Ui) -> Finished {
        let Running {
            label,
            mut child,
            stderr,
            started,
            tools,
            end,
            ..
        } = r;
        let status = child.wait().ok();
        let stderr = stderr.join().unwrap_or_default();
        let secs = started.elapsed().as_secs();
        let summary = format!(
            "{} · {secs}s",
            match tools {
                1 => "1 tool call".to_string(),
                n => format!("{n} tool calls"),
            }
        );
        let session = end
            .as_ref()
            .and_then(|v| v["session"].as_str())
            .map(String::from);
        let mut notes: String = plan
            .notes
            .iter()
            .map(|n| format!("\n[note: {n}]"))
            .collect();
        if let Some(path) = &session {
            notes.push_str(&format!("\n[sub-agent transcript: {path}]"));
        }
        let out = match end {
            Some(v) if v["type"] == "result" => {
                let mut text = v["text"].as_str().unwrap_or("").to_string();
                if v["truncated"] == true {
                    text.push_str(
                        "\n[answer cut at the result limit; the transcript has all of it]",
                    );
                }
                Ok(format!("{}\n{notes}", text.trim_end()))
            }
            Some(v) => Err(format!(
                "the sub-agent failed: {}{notes}",
                v["message"].as_str().unwrap_or("unknown error")
            )),
            None => Err(format!(
                "the sub-agent ended without an answer ({}){}{notes}",
                status.map_or("no status".into(), |s| s.to_string()),
                match stderr.trim() {
                    "" => String::new(),
                    e => format!(": {e}"),
                }
            )),
        };
        ui.agent_done(&label, &out, &summary);
        Finished {
            result: out,
            session: session.map(std::path::PathBuf::from),
        }
    }
}

/// One line for the spinner: what each running sub-agent is doing.
fn progress(running: &[Running]) -> String {
    let each: Vec<String> = running
        .iter()
        .map(|r| match r.doing.as_str() {
            "" => r.label.clone(),
            d => format!("{}: {d}", r.label),
        })
        .collect();
    match each.len() {
        1 => each[0].clone(),
        n => format!("{n} agents · {}", each.join(" · ")),
    }
}

/// Closes each child's input, which asks it to stop its turn; children that
/// do not exit within two seconds are killed with their process group.
fn stop(running: &mut [Running]) {
    for r in running.iter_mut() {
        r.stdin.take();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline
        && running
            .iter_mut()
            .any(|r| !matches!(r.child.try_wait(), Ok(Some(_))))
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    for r in running.iter_mut() {
        if !matches!(r.child.try_wait(), Ok(Some(_))) {
            crate::tools::kill(&mut r.child);
        }
        let _ = r.child.wait();
    }
}

/// A sub-agent's result and the transcript it left.
pub struct Finished {
    pub result: Result<String, String>,
    pub session: Option<std::path::PathBuf>,
}
