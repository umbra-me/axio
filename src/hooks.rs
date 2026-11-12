//! Hooks: commands run at fixed points, in the wire format Claude Code, Codex
//! and Grok Build share. A hook reads one JSON object on stdin and answers
//! through its exit code and optional JSON on stdout.

use serde_json::{Map, Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    PreCompact,
    SessionEnd,
    SubagentStart,
    SubagentStop,
}

impl Event {
    pub const ALL: [Event; 9] = [
        Event::SessionStart,
        Event::UserPromptSubmit,
        Event::PreToolUse,
        Event::PostToolUse,
        Event::Stop,
        Event::PreCompact,
        Event::SessionEnd,
        Event::SubagentStart,
        Event::SubagentStop,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Event::SessionStart => "SessionStart",
            Event::UserPromptSubmit => "UserPromptSubmit",
            Event::PreToolUse => "PreToolUse",
            Event::PostToolUse => "PostToolUse",
            Event::Stop => "Stop",
            Event::PreCompact => "PreCompact",
            Event::SessionEnd => "SessionEnd",
            Event::SubagentStart => "SubagentStart",
            Event::SubagentStop => "SubagentStop",
        }
    }

    fn from_name(name: &str) -> Option<Event> {
        Event::ALL.into_iter().find(|e| e.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Hook {
    event: Event,
    matcher: String,
    command: String,
    timeout: Duration,
    /// The plugin folder, for `${CLAUDE_PLUGIN_ROOT}`.
    root: Option<PathBuf>,
}

/// Hooks declared somewhere other than a known event or command are reported
/// once and skipped, so a config written for a newer harness still loads.
#[derive(Clone, Debug, Default)]
pub struct Hooks {
    hooks: Vec<Hook>,
    pub skipped: Vec<String>,
    pub project_dir: PathBuf,
}

/// Claude Code matchers are regexes on the tool name. Without a regex
/// dependency axio supports the forms hooks use in practice: empty or `*`
/// for every tool, `A|B` alternatives, exact names, and a trailing `.*` or
/// `*` for a prefix such as `mcp__github__.*`.
fn matches(matcher: &str, tool: &str) -> bool {
    let m = matcher.trim();
    if m.is_empty() || m == "*" || m == ".*" {
        return true;
    }
    m.split('|').any(|alt| {
        let alt = alt.trim();
        match alt.strip_suffix(".*").or_else(|| alt.strip_suffix('*')) {
            Some(prefix) => tool.starts_with(prefix),
            None => alt == tool,
        }
    })
}

impl Hooks {
    /// Adds the hooks in a `{"hooks": {...}}` document from `source`.
    pub fn add(&mut self, doc: &Value, root: Option<&Path>, source: &Path) {
        let Some(events) = doc.get("hooks").and_then(Value::as_object) else {
            return;
        };
        for (name, groups) in events {
            let Some(event) = Event::from_name(name) else {
                self.skipped
                    .push(format!("{}: {name} has no axio event", source.display()));
                continue;
            };
            for group in groups.as_array().into_iter().flatten() {
                let matcher = group["matcher"].as_str().unwrap_or("").to_string();
                for h in group["hooks"].as_array().into_iter().flatten() {
                    let (Some("command"), Some(command)) =
                        (h["type"].as_str(), h["command"].as_str())
                    else {
                        self.skipped.push(format!(
                            "{}: a {name} hook is not a command hook",
                            source.display()
                        ));
                        continue;
                    };
                    let timeout = h["timeout"]
                        .as_u64()
                        .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
                    self.hooks.push(Hook {
                        event,
                        matcher: matcher.clone(),
                        command: command.to_string(),
                        timeout,
                        root: root.map(Path::to_path_buf),
                    });
                }
            }
        }
    }

    /// Reads a settings or `hooks.json` file; a missing file adds nothing.
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

    pub fn new(project_dir: PathBuf) -> Hooks {
        Hooks {
            project_dir,
            ..Hooks::default()
        }
    }

    pub fn has(&self, event: Event) -> bool {
        self.hooks.iter().any(|h| h.event == event)
    }

    /// Runs every hook for `event` whose matcher fits `tool`, in parallel,
    /// with `input` plus the event name on stdin.
    pub fn run(&self, event: Event, tool: Option<&str>, mut input: Map<String, Value>) -> Outcome {
        let selected: Vec<&Hook> = self
            .hooks
            .iter()
            .filter(|h| h.event == event && tool.is_none_or(|t| matches(&h.matcher, t)))
            .collect();
        if selected.is_empty() {
            return Outcome::default();
        }
        input.insert("hook_event_name".into(), json!(event.name()));
        let stdin = Value::Object(input).to_string();
        let replies: Vec<Reply> = std::thread::scope(|s| {
            let handles: Vec<_> = selected
                .iter()
                .map(|h| s.spawn(|| run_one(h, &stdin, &self.project_dir)))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Reply::failed("hook thread panicked"))
                })
                .collect()
        });
        Outcome::merge(event, replies)
    }
}

#[derive(Debug, Default, PartialEq)]
struct Reply {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    error: Option<String>,
}

impl Reply {
    fn failed(e: &str) -> Reply {
        Reply {
            error: Some(e.into()),
            ..Reply::default()
        }
    }
}

fn expand(text: &str, root: Option<&Path>, project: &Path) -> String {
    let mut out = text.replace("${AXIO_PROJECT_DIR}", &project.display().to_string());
    out = out.replace("${CLAUDE_PROJECT_DIR}", &project.display().to_string());
    if let Some(r) = root {
        let r = r.display().to_string();
        out = out.replace("${CLAUDE_PLUGIN_ROOT}", &r);
        out = out.replace("${AXIO_PLUGIN_ROOT}", &r);
    }
    out
}

fn run_one(hook: &Hook, stdin: &str, project: &Path) -> Reply {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(expand(&hook.command, hook.root.as_deref(), project))
        .current_dir(project)
        .env("AXIO_PROJECT_DIR", project)
        .env("CLAUDE_PROJECT_DIR", project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(r) = &hook.root {
        cmd.env("AXIO_PLUGIN_ROOT", r).env("CLAUDE_PLUGIN_ROOT", r);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Reply::failed(&format!("could not start `{}`: {e}", hook.command)),
    };
    // Write and read on threads so a hook that ignores stdin or floods
    // stdout cannot block the other pipe.
    let mut input = child.stdin.take().unwrap();
    let data = stdin.to_string();
    let writer = std::thread::spawn(move || {
        let _ = input.write_all(data.as_bytes());
    });
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = pipe.read_to_string(&mut s);
            s
        })
    };
    let out = read(Box::new(child.stdout.take().unwrap()));
    let err = read(Box::new(child.stderr.take().unwrap()));
    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() < hook.timeout => {
                std::thread::sleep(Duration::from_millis(5))
            }
            _ => {
                #[cfg(unix)]
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                return Reply::failed(&format!(
                    "`{}` timed out after {}s",
                    hook.command,
                    hook.timeout.as_secs()
                ));
            }
        }
    };
    let _ = writer.join();
    Reply {
        code,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
        error: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Permission {
    Allow,
    Ask(String),
    Deny(String),
}

/// What the hooks for one event decided together.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Blocked, with the reason the model or user should see.
    pub block: Option<String>,
    /// `continue: false`: end the turn, with the reason for the user.
    pub stop: Option<String>,
    /// Text for the model, appended to the conversation.
    pub context: Vec<String>,
    /// Text for the user only.
    pub messages: Vec<String>,
    pub permission: Option<Permission>,
    pub updated_input: Option<Value>,
}

impl Outcome {
    fn merge(event: Event, replies: Vec<Reply>) -> Outcome {
        let mut o = Outcome::default();
        for r in replies {
            if let Some(e) = r.error {
                o.messages.push(format!("hook failed: {e}"));
                continue;
            }
            match r.code {
                Some(0) => {}
                // Exit 2 blocks, with stderr as the reason.
                Some(2) => {
                    let reason = r.stderr.trim();
                    o.block.get_or_insert_with(|| {
                        if reason.is_empty() {
                            "blocked by a hook".into()
                        } else {
                            reason.into()
                        }
                    });
                    continue;
                }
                code => {
                    let detail = r.stderr.trim();
                    o.messages.push(format!(
                        "hook exited with {}{}{detail}",
                        code.map_or("a signal".into(), |c| c.to_string()),
                        if detail.is_empty() { "" } else { ": " }
                    ));
                    continue;
                }
            }
            let out = r.stdout.trim();
            let Ok(j @ Value::Object(_)) = serde_json::from_str::<Value>(out) else {
                // Plain stdout is context only where Claude Code treats it so.
                if !out.is_empty() && matches!(event, Event::SessionStart | Event::UserPromptSubmit)
                {
                    o.context.push(out.to_string());
                }
                continue;
            };
            if j.get("continue") == Some(&Value::Bool(false)) {
                o.stop.get_or_insert_with(|| {
                    j["stopReason"]
                        .as_str()
                        .unwrap_or("stopped by a hook")
                        .to_string()
                });
            }
            if let Some(m) = j["systemMessage"].as_str() {
                o.messages.push(m.to_string());
            }
            if j["decision"] == "block" {
                o.block.get_or_insert_with(|| {
                    j["reason"]
                        .as_str()
                        .unwrap_or("blocked by a hook")
                        .to_string()
                });
            }
            let specific = &j["hookSpecificOutput"];
            if let Some(c) = specific["additionalContext"].as_str() {
                o.context.push(c.to_string());
            }
            let reason = specific["permissionDecisionReason"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let p = match specific["permissionDecision"].as_str() {
                Some("deny") => Some(Permission::Deny(reason)),
                Some("ask") => Some(Permission::Ask(reason)),
                Some("allow") => Some(Permission::Allow),
                _ => None,
            };
            // The strictest answer wins: deny, then ask, then allow.
            o.permission = match (o.permission.take(), p) {
                (Some(Permission::Deny(r)), _) | (_, Some(Permission::Deny(r))) => {
                    Some(Permission::Deny(r))
                }
                (Some(Permission::Ask(r)), _) | (_, Some(Permission::Ask(r))) => {
                    Some(Permission::Ask(r))
                }
                (a, b) => b.or(a),
            };
            if !specific["updatedInput"].is_null() {
                o.updated_input = Some(specific["updatedInput"].clone());
            }
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_agent_events_load() {
        let mut h = Hooks::new(std::env::temp_dir());
        h.add(
            &serde_json::json!({"hooks": {
                "SubagentStart": [{"matcher": "explore", "hooks": [{"type": "command", "command": "true"}]}],
                "SubagentStop": [{"hooks": [{"type": "command", "command": "true"}]}]
            }}),
            None,
            Path::new("t"),
        );
        assert!(h.skipped.is_empty(), "{:?}", h.skipped);
        assert!(h.has(Event::SubagentStart) && h.has(Event::SubagentStop));
    }

    fn hooks(event: &str, matcher: &str, command: &str) -> Hooks {
        let mut h = Hooks {
            project_dir: std::env::temp_dir(),
            ..Hooks::default()
        };
        h.add(
            &json!({"hooks": {event: [{"matcher": matcher, "hooks": [
                {"type": "command", "command": command, "timeout": 5}
            ]}]}}),
            None,
            Path::new("test"),
        );
        h
    }

    fn tool_input(tool: &str, command: &str) -> Map<String, Value> {
        json!({"tool_name": tool, "tool_input": {"command": command}})
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn matchers_cover_the_common_forms() {
        assert!(matches("", "Bash") && matches("*", "Bash") && matches(".*", "Edit"));
        assert!(matches("Bash|Edit", "Edit") && !matches("Bash|Edit", "Read"));
        assert!(matches("mcp__github__.*", "mcp__github__create_issue"));
        assert!(!matches("Bash", "BashX"));
    }

    #[test]
    fn exit_2_blocks_with_stderr_and_reads_the_input() {
        let h = hooks(
            "PreToolUse",
            "Bash",
            r#"grep -q 'rm -rf' && { echo "no recursive delete" >&2; exit 2; }; exit 0"#,
        );
        let o = h.run(
            Event::PreToolUse,
            Some("Bash"),
            tool_input("Bash", "rm -rf /"),
        );
        assert_eq!(o.block.as_deref(), Some("no recursive delete"));
        let o = h.run(Event::PreToolUse, Some("Bash"), tool_input("Bash", "ls"));
        assert_eq!(o, Outcome::default());
        let o = h.run(
            Event::PreToolUse,
            Some("Read"),
            tool_input("Read", "rm -rf /"),
        );
        assert_eq!(o, Outcome::default(), "the matcher excludes Read");
    }

    #[test]
    fn json_replies_set_permission_context_and_rewrites() {
        let h = hooks(
            "PreToolUse",
            "",
            r#"echo '{"systemMessage":"checked","hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"deploys","updatedInput":{"command":"echo safe"},"additionalContext":"note"}}'"#,
        );
        let o = h.run(
            Event::PreToolUse,
            Some("Bash"),
            tool_input("Bash", "deploy"),
        );
        assert_eq!(o.permission, Some(Permission::Ask("deploys".into())));
        assert_eq!(o.updated_input, Some(json!({"command": "echo safe"})));
        assert_eq!(
            (o.context, o.messages),
            (vec!["note".to_string()], vec!["checked".to_string()])
        );
    }

    #[test]
    fn deny_beats_ask_beats_allow_across_hooks() {
        let mut h = hooks(
            "PreToolUse",
            "",
            r#"echo '{"hookSpecificOutput":{"permissionDecision":"allow"}}'"#,
        );
        h.add(
            &json!({"hooks": {"PreToolUse": [{"hooks": [
                {"type": "command", "command": "echo '{\"hookSpecificOutput\":{\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"no\"}}'"},
                {"type": "command", "command": "echo '{\"hookSpecificOutput\":{\"permissionDecision\":\"ask\"}}'"}
            ]}]}}),
            None,
            Path::new("second"),
        );
        let o = h.run(Event::PreToolUse, Some("Bash"), Map::new());
        assert_eq!(o.permission, Some(Permission::Deny("no".into())));
    }

    #[test]
    fn plain_stdout_is_context_for_prompt_events_only() {
        let h = hooks(
            "UserPromptSubmit",
            "",
            "cat >/dev/null; echo 'today is Tuesday'",
        );
        assert_eq!(
            h.run(Event::UserPromptSubmit, None, Map::new()).context,
            ["today is Tuesday"]
        );
        let h = hooks("PostToolUse", "", "echo 'ignored'");
        assert!(
            h.run(Event::PostToolUse, Some("Bash"), Map::new())
                .context
                .is_empty()
        );
    }

    #[test]
    fn stop_block_continue_and_failures() {
        let h = hooks(
            "Stop",
            "",
            r#"echo '{"decision":"block","reason":"run the tests"}'"#,
        );
        assert_eq!(
            h.run(Event::Stop, None, Map::new()).block.as_deref(),
            Some("run the tests")
        );
        let h = hooks(
            "Stop",
            "",
            r#"echo '{"continue":false,"stopReason":"quota"}'"#,
        );
        assert_eq!(
            h.run(Event::Stop, None, Map::new()).stop.as_deref(),
            Some("quota")
        );
        let h = hooks("Stop", "", "echo oops >&2; exit 1");
        assert_eq!(
            h.run(Event::Stop, None, Map::new()).messages,
            ["hook exited with 1: oops"]
        );
        let mut slow = hooks("Stop", "", "sleep 5");
        slow.hooks[0].timeout = Duration::from_millis(200);
        let started = Instant::now();
        assert!(slow.run(Event::Stop, None, Map::new()).messages[0].contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn plugin_root_and_unknown_events() {
        let dir = std::env::temp_dir().join(format!("axio-hooks-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("guard/hooks")).unwrap();
        std::fs::write(
            dir.join("guard/hooks/hooks.json"),
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo \"{\\\"decision\\\":\\\"block\\\",\\\"reason\\\":\\\"$CLAUDE_PLUGIN_ROOT\\\"}\""}]}],
                         "PermissionRequest": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
        )
        .unwrap();
        let mut h = Hooks {
            project_dir: dir.clone(),
            ..Hooks::default()
        };
        for p in crate::plugins::list(&dir, None) {
            h.add_file(&p.root.join("hooks/hooks.json"), Some(&p.root))
                .unwrap();
        }
        assert_eq!(h.skipped.len(), 1);
        assert!(h.skipped[0].ends_with("PermissionRequest has no axio event"));
        let o = h.run(Event::Stop, None, Map::new());
        assert_eq!(o.block, Some(dir.join("guard").display().to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_example_guard_plugin_decides() {
        let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins");
        let mut h = Hooks::new(std::env::temp_dir());
        for p in crate::plugins::list(&plugins, None) {
            h.add_file(&p.root.join("hooks/hooks.json"), Some(&p.root))
                .unwrap();
        }
        let decide = |cmd: &str| {
            h.run(Event::PreToolUse, Some("Bash"), tool_input("Bash", cmd))
                .permission
        };
        assert_eq!(
            decide("rm -rf ~"),
            Some(Permission::Deny(
                "recursive delete of a home or root directory".into()
            ))
        );
        assert_eq!(
            decide("git push --force origin main"),
            Some(Permission::Deny("force push".into()))
        );
        assert_eq!(
            decide("git push origin topic"),
            Some(Permission::Ask("pushes to a remote".into()))
        );
        assert_eq!(decide("rm -rf target"), None);
        assert_eq!(decide("ls"), None);
    }
}
