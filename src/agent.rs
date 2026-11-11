use crate::compact;
use crate::hooks::{self, Event as Hook, Hooks, Permission};
use crate::mcp::Mcp;
use crate::message::{Block, Message};
use crate::provider::{self, Delta, Model, Reply, Request, Stop, Usage};
use crate::session::Session;
use crate::tools::{self, Modules, ToolSpec};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// The error a turn ends with when the user stops it.
pub const ABORTED: &str = "aborted";

const SUMMARY_SYSTEM: &str = "You write compaction summaries of a coding agent's session.";

pub trait Ui {
    fn delta(&mut self, d: Delta);
    fn tool_start(&mut self, name: &str, input: &Value);
    fn tool_end(&mut self, result: &Result<String, String>);
    fn stopped(&mut self, stop: &Stop);
    fn usage(&mut self, u: Usage);
    /// Whether the user asked to stop the running turn; polled often.
    fn interrupted(&mut self) -> bool {
        false
    }
    fn notice(&mut self, _text: &str) {}
    /// The agent is waiting on the model; `label` says for what.
    fn waiting(&mut self, _label: &str) {}
    /// A yes/no question for the user; without a terminal the answer is no.
    fn ask(&mut self, _question: &str) -> bool {
        false
    }
}

/// How many times Stop hooks may send the model back to work in one turn.
const MAX_STOP_CONTINUATIONS: usize = 8;

pub type Complete = dyn Fn(&Request, &mut dyn FnMut(Delta)) -> Result<Reply, String> + Send + Sync;

pub struct Agent {
    pub model: Model,
    pub effort: Option<String>,
    pub cwd: PathBuf,
    pub system: String,
    pub messages: Vec<Message>,
    pub session: Option<Session>,
    /// Context size after the latest reply, in tokens.
    pub context: u64,
    pub hooks: Hooks,
    pub modules: Modules,
    /// Fraction of the context window at which compaction starts.
    pub compact_ratio: f64,
    mcp: Arc<Mcp>,
    tools: Vec<ToolSpec>,
    complete: Arc<Complete>,
}

enum Event {
    Text(String),
    Thinking,
    ToolCall,
    Done(Result<Reply, String>),
}

impl Agent {
    pub fn new(model: Model, cwd: PathBuf, system: String) -> Agent {
        Agent {
            model,
            effort: None,
            cwd,
            system,
            messages: Vec::new(),
            session: None,
            context: 0,
            hooks: Hooks::default(),
            modules: Modules::default(),
            compact_ratio: 0.85,
            mcp: Arc::default(),
            tools: tools::specs(),
            complete: Arc::new(provider::complete),
        }
    }

    /// The core tools, the profile's modules and started MCP servers' tools.
    pub fn set_tools(&mut self, modules: Modules, mcp: Mcp) {
        self.modules = modules;
        self.tools = tools::specs_for(modules);
        self.tools.extend(mcp.specs.iter().cloned());
        self.mcp = Arc::new(mcp);
    }

    /// Compaction starts at 85% of the window, or AXIO_COMPACT_AT tokens.
    pub fn compact_at(&self) -> u64 {
        std::env::var("AXIO_COMPACT_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or((self.model.context_window() as f64 * self.compact_ratio) as u64)
    }

    fn push(&mut self, m: Message) -> Result<(), String> {
        if let Some(s) = &mut self.session {
            s.append(&m)?;
        }
        compact::append(&mut self.messages, m);
        Ok(())
    }

    /// Runs one model call on a worker thread so the user can stop it at once;
    /// the stream sees the cancel flag and closes the connection.
    fn call(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Vec<ToolSpec>,
        stream: bool,
        ui: &mut dyn Ui,
    ) -> Result<Reply, String> {
        ui.waiting(if stream {
            "thinking"
        } else {
            "summarizing the conversation"
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (complete, model, system, effort, flag) = (
            self.complete.clone(),
            self.model.clone(),
            system.to_string(),
            self.effort.clone(),
            cancel.clone(),
        );
        std::thread::spawn(move || {
            let req = Request {
                model: &model,
                system: &system,
                messages: &messages,
                tools: &tools,
                effort: effort.as_deref(),
                cancel: &flag,
            };
            let deltas = tx.clone();
            let result = complete(&req, &mut |d| {
                let _ = deltas.send(match d {
                    Delta::Text(t) => Event::Text(t.to_string()),
                    Delta::Thinking => Event::Thinking,
                    Delta::ToolCall => Event::ToolCall,
                });
            });
            let _ = tx.send(Event::Done(result));
        });
        loop {
            if ui.interrupted() {
                cancel.store(true, Ordering::Relaxed);
                return Err(ABORTED.into());
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Event::Done(result)) => return result,
                Ok(_) if !stream => {}
                Ok(Event::Text(t)) => ui.delta(Delta::Text(&t)),
                Ok(Event::Thinking) => ui.delta(Delta::Thinking),
                Ok(Event::ToolCall) => ui.delta(Delta::ToolCall),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the model call ended without a reply".into());
                }
            }
        }
    }

    /// Runs the hooks for `event` with the fields every hook receives, and
    /// shows the user any messages they returned.
    fn hook(
        &self,
        event: Hook,
        tool: Option<&str>,
        fields: Value,
        ui: &mut dyn Ui,
    ) -> hooks::Outcome {
        if !self.hooks.has(event) {
            return hooks::Outcome::default();
        }
        let mut input: Map<String, Value> = fields.as_object().cloned().unwrap_or_default();
        let transcript = self.session.as_ref().map(|s| s.path.clone());
        let id = transcript
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or("unsaved".into(), |s| s.to_string_lossy().into_owned());
        input.insert("session_id".into(), json!(id));
        input.insert(
            "transcript_path".into(),
            json!(transcript.map(|p| p.display().to_string())),
        );
        input.insert("cwd".into(), json!(self.cwd.display().to_string()));
        input.insert("model".into(), json!(self.model.to_string()));
        input.insert("permission_mode".into(), json!("default"));
        let outcome = self.hooks.run(event, tool, input);
        for m in &outcome.messages {
            ui.notice(m);
        }
        outcome
    }

    /// SessionStart: `source` is "startup" or "resume". Context the hooks
    /// return opens the conversation.
    pub fn start(&mut self, source: &str, ui: &mut dyn Ui) -> Result<(), String> {
        let o = self.hook(Hook::SessionStart, None, json!({"source": source}), ui);
        for c in o.context {
            self.push(Message::user_text(c))?;
        }
        Ok(())
    }

    pub fn end(&self, reason: &str, ui: &mut dyn Ui) {
        self.hook(Hook::SessionEnd, None, json!({"reason": reason}), ui);
    }

    /// Replaces the history with recent requests and a summary. Returns
    /// whether there was anything to compact. `trigger` is "manual" or "auto".
    pub fn compact(&mut self, trigger: &str, ui: &mut dyn Ui) -> Result<bool, String> {
        if !compact::worthwhile(&self.messages) {
            return Ok(false);
        }
        let pre = self.hook(
            Hook::PreCompact,
            None,
            json!({"trigger": trigger, "custom_instructions": ""}),
            ui,
        );
        if let Some(reason) = pre.block.or(pre.stop) {
            ui.notice(&format!("compaction skipped by a hook: {reason}"));
            return Ok(false);
        }
        ui.notice("compacting the conversation…");
        let mut instruction = compact::INSTRUCTION.to_string();
        for c in pre.context {
            instruction.push_str("\n\n");
            instruction.push_str(&c);
        }
        let prompt = format!(
            "<transcript>\n{}</transcript>\n\n{instruction}",
            compact::transcript(&self.messages),
        );
        let reply = self.call(
            SUMMARY_SYSTEM,
            vec![Message::user_text(prompt)],
            Vec::new(),
            false,
            ui,
        )?;
        let summary = compact::summary_text(
            &Message::Assistant {
                content: reply.content,
            }
            .text(),
        );
        if summary.is_empty() {
            return Err("the model returned an empty summary".into());
        }
        compact::apply(&mut self.messages, &summary);
        if let Some(s) = &mut self.session {
            s.compaction(&summary)?;
        }
        self.context = 0;
        Ok(true)
    }

    /// Runs one user prompt to completion: model replies and tool calls repeat
    /// until the model answers without calling a tool. A stopped turn returns
    /// `Err(ABORTED)` and leaves a history the provider still accepts.
    pub fn prompt(&mut self, text: &str, ui: &mut dyn Ui) -> Result<(), String> {
        let submit = self.hook(Hook::UserPromptSubmit, None, json!({"prompt": text}), ui);
        if let Some(reason) = submit.block.or(submit.stop) {
            ui.notice(&format!("prompt blocked by a hook: {reason}"));
            return Ok(());
        }
        if self.context >= self.compact_at() {
            match self.compact("auto", ui) {
                Err(e) if e == ABORTED => return Err(e),
                Err(e) => ui.notice(&format!("compaction failed: {e}")),
                Ok(_) => {}
            }
        }
        let mut content = vec![Block::Text { text: text.into() }];
        content.extend(submit.context.into_iter().map(|text| Block::Text { text }));
        self.push(Message::User { content })?;
        let result = self.run_turn(ui);
        if matches!(&result, Err(e) if e == ABORTED) {
            self.push(Message::user_text(compact::INTERRUPTED))?;
        }
        result
    }

    fn run_turn(&mut self, ui: &mut dyn Ui) -> Result<(), String> {
        let mut continuations = 0;
        loop {
            if self.modules.background {
                for note in crate::background::finished() {
                    ui.notice(&note);
                    self.push(Message::user_text(note))?;
                }
            }
            let reply = self.call(
                &self.system,
                self.messages.clone(),
                self.tools.clone(),
                true,
                ui,
            )?;
            let (msg, stop, usage) = reply.into_message();
            self.context = usage.context();
            ui.usage(usage);
            let calls: Vec<(String, String, Value)> = msg
                .tool_calls()
                .into_iter()
                .map(|(id, name, input)| (id.into(), name.into(), input.clone()))
                .collect();
            self.push(msg)?;
            if calls.is_empty() {
                let active = continuations > 0;
                let o = self.hook(Hook::Stop, None, json!({"stop_hook_active": active}), ui);
                if let (Some(reason), None) = (&o.block, &o.stop)
                    && continuations < MAX_STOP_CONTINUATIONS
                {
                    continuations += 1;
                    ui.notice(&format!("a Stop hook asked for more work: {reason}"));
                    self.push(Message::user_text(format!("[Stop hook] {reason}")))?;
                    continue;
                }
                if let Some(reason) = o.stop {
                    ui.notice(&format!("stopped by a hook: {reason}"));
                }
                ui.stopped(&stop);
                return Ok(());
            }
            let mut results = Vec::new();
            let mut stopped = false;
            let mut hook_stop: Option<String> = None;
            for (id, name, input) in calls {
                stopped = stopped || ui.interrupted();
                // A reply cut off at max_tokens can end inside a tool call.
                let result = if stopped {
                    Err("not run: the user stopped the turn".into())
                } else if let Some(reason) = &hook_stop {
                    Err(format!("not run: a hook ended the turn ({reason})"))
                } else if stop == Stop::MaxTokens {
                    Err("the reply hit the output limit before this call was complete".into())
                } else {
                    let (r, end) = self.run_tool(&id, &name, input, ui);
                    stopped = matches!(&r, Err(e) if e.ends_with(tools::STOPPED));
                    hook_stop = end;
                    r
                };
                let (output, is_error) = match result {
                    Ok(o) => (o, false),
                    Err(e) => (e, true),
                };
                results.push(Block::ToolResult {
                    id,
                    output,
                    is_error,
                });
            }
            self.push(Message::User { content: results })?;
            if stopped {
                return Err(ABORTED.into());
            }
            if let Some(reason) = hook_stop {
                ui.notice(&format!("stopped by a hook: {reason}"));
                ui.stopped(&Stop::EndTurn);
                return Ok(());
            }
        }
    }

    /// One tool call with its PreToolUse and PostToolUse hooks. Returns the
    /// result and, when a hook ended the turn, its reason.
    fn run_tool(
        &self,
        id: &str,
        name: &str,
        mut input: Value,
        ui: &mut dyn Ui,
    ) -> (Result<String, String>, Option<String>) {
        let fields =
            |input: &Value| json!({"tool_name": name, "tool_input": input, "tool_use_id": id});
        let pre = self.hook(Hook::PreToolUse, Some(name), fields(&input), ui);
        if let Some(reason) = pre.stop {
            return (
                Err(format!("not run: a hook ended the turn ({reason})")),
                Some(reason),
            );
        }
        if let Some(changed) = pre.updated_input {
            input = changed;
        }
        let refusal = match (pre.block, pre.permission) {
            (Some(reason), _) | (_, Some(Permission::Deny(reason))) => {
                Some(format!("blocked by a hook: {reason}"))
            }
            (None, Some(Permission::Ask(reason))) => {
                let why = if reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({reason})")
                };
                let summary = input["command"]
                    .as_str()
                    .or(input["file_path"].as_str())
                    .unwrap_or("");
                (!ui.ask(&format!("Allow {name} {summary}{why}?")))
                    .then(|| format!("the user declined this call{why}"))
            }
            _ => None,
        };
        let mut result = match refusal {
            Some(r) => {
                // Show refused calls too, so the user sees what was asked.
                ui.tool_start(name, &input);
                let r = Err(r);
                ui.tool_end(&r);
                r
            }
            None => {
                ui.tool_start(name, &input);
                let r = if self.mcp.owns(name) {
                    self.mcp.call(name, &input, &mut || ui.interrupted())
                } else {
                    tools::run_with(self.modules, name, &input, &self.cwd, &mut || {
                        ui.interrupted()
                    })
                };
                ui.tool_end(&r);
                r
            }
        };
        let mut extra: Vec<String> = pre.context;
        let (text, is_error) = match &result {
            Ok(t) => (t.clone(), false),
            Err(e) => (e.clone(), true),
        };
        let post = self.hook(
            Hook::PostToolUse,
            Some(name),
            json!({"tool_name": name, "tool_input": input, "tool_use_id": id,
                   "tool_response": {"output": text, "is_error": is_error}}),
            ui,
        );
        if let Some(reason) = post.block {
            extra.push(format!("[PostToolUse hook] {reason}"));
        }
        extra.extend(post.context);
        if !extra.is_empty() {
            let add = |t: &mut String| {
                for e in &extra {
                    t.push_str("\n\n");
                    t.push_str(e);
                }
            };
            match &mut result {
                Ok(t) | Err(t) => add(t),
            }
        }
        (result, post.stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Log {
        events: Vec<String>,
        stop_after_polls: Option<usize>,
        polls: usize,
        answer: bool,
    }
    impl Ui for Log {
        fn delta(&mut self, d: Delta) {
            if let Delta::Text(t) = d {
                self.events.push(format!("text {t}"));
            }
        }
        fn tool_start(&mut self, name: &str, _: &Value) {
            self.events.push(format!("tool {name}"));
        }
        fn tool_end(&mut self, r: &Result<String, String>) {
            self.events.push(format!("ok {}", r.is_ok()));
        }
        fn stopped(&mut self, s: &Stop) {
            self.events.push(format!("stop {s:?}"));
        }
        fn usage(&mut self, _: Usage) {}
        fn interrupted(&mut self) -> bool {
            self.polls += 1;
            self.stop_after_polls.is_some_and(|n| self.polls > n)
        }
        fn notice(&mut self, text: &str) {
            self.events.push(text.to_string());
        }
        fn ask(&mut self, q: &str) -> bool {
            self.events.push(format!("ask {q}"));
            self.answer
        }
    }

    fn with_hooks(agent: &mut Agent, doc: Value) {
        let mut h = Hooks::new(std::env::temp_dir());
        h.add(&doc, None, std::path::Path::new("test"));
        agent.hooks = h;
    }

    fn hook(event: &str, matcher: &str, command: &str) -> Value {
        json!({"hooks": {event: [{"matcher": matcher, "hooks": [{"type": "command", "command": command}]}]}})
    }

    /// Calls `command` with Bash once, then answers with the last tool result.
    fn bash_then_echo(command: &'static str) -> Agent {
        let turn = Mutex::new(0);
        agent(move |req, _| {
            let mut turn = turn.lock().unwrap();
            *turn += 1;
            if *turn == 1 {
                return reply(bash("t1", command), Stop::ToolUse, 1);
            }
            let Message::User { content } = req.messages.last().unwrap() else {
                panic!()
            };
            let Block::ToolResult { output, .. } = &content[0] else {
                panic!()
            };
            reply(
                vec![Block::Text {
                    text: output.clone(),
                }],
                Stop::EndTurn,
                1,
            )
        })
    }

    #[test]
    fn pre_tool_use_hooks_deny_ask_and_rewrite() {
        let mut a = bash_then_echo("echo 42");
        with_hooks(
            &mut a,
            hook(
                "PreToolUse",
                "Bash",
                r#"echo '{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"read-only day"}}'"#,
            ),
        );
        let mut log = Log::default();
        a.prompt("go", &mut log).unwrap();
        assert_eq!(
            a.messages.last().unwrap().text(),
            "blocked by a hook: read-only day"
        );
        assert_eq!(
            log.events[..2],
            ["tool Bash", "ok false"],
            "refused calls are shown"
        );

        let mut a = bash_then_echo("echo 42");
        with_hooks(
            &mut a,
            hook(
                "PreToolUse",
                "",
                r#"echo '{"hookSpecificOutput":{"permissionDecision":"ask"}}'"#,
            ),
        );
        let mut log = Log::default();
        a.prompt("go", &mut log).unwrap();
        assert!(log.events.contains(&"ask Allow Bash echo 42?".to_string()));
        assert_eq!(
            a.messages.last().unwrap().text(),
            "the user declined this call"
        );

        let mut a = bash_then_echo("echo 42");
        with_hooks(
            &mut a,
            hook(
                "PreToolUse",
                "Bash",
                r#"echo '{"hookSpecificOutput":{"permissionDecision":"allow","updatedInput":{"command":"echo 7"}}}'"#,
            ),
        );
        a.prompt(
            "go",
            &mut Log {
                answer: true,
                ..Log::default()
            },
        )
        .unwrap();
        assert_eq!(a.messages.last().unwrap().text(), "7");
    }

    #[test]
    fn post_tool_use_context_reaches_the_model() {
        let mut a = bash_then_echo("echo 42");
        with_hooks(
            &mut a,
            hook(
                "PostToolUse",
                "Bash",
                r#"echo '{"decision":"block","reason":"output looks wrong","hookSpecificOutput":{"additionalContext":"lint passed"}}'"#,
            ),
        );
        a.prompt("go", &mut Log::default()).unwrap();
        assert_eq!(
            a.messages.last().unwrap().text(),
            "42\n\n[PostToolUse hook] output looks wrong\n\nlint passed"
        );
    }

    #[test]
    fn stop_hooks_send_the_model_back_until_satisfied() {
        let calls = Arc::new(Mutex::new(0));
        let seen = calls.clone();
        let mut a = agent(move |req, _| {
            *seen.lock().unwrap() += 1;
            let last = req.messages.last().unwrap().text();
            reply(
                vec![Block::Text {
                    text: format!("re: {last}"),
                }],
                Stop::EndTurn,
                1,
            )
        });
        let marker = std::env::temp_dir().join(format!("axio-stop-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let cmd: &'static str = Box::leak(format!(
            r#"test -e {m} && exit 0; touch {m}; echo '{{"decision":"block","reason":"run the tests"}}'"#,
            m = marker.display()
        ).into_boxed_str());
        with_hooks(&mut a, hook("Stop", "", cmd));
        a.prompt("fix it", &mut Log::default()).unwrap();
        assert_eq!(*calls.lock().unwrap(), 2);
        assert_eq!(
            a.messages.last().unwrap().text(),
            "re: [Stop hook] run the tests"
        );
        std::fs::remove_file(marker).unwrap();

        // A hook that never relents is capped.
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let mut a = agent(move |_, _| {
            *seen.lock().unwrap() += 1;
            reply(
                vec![Block::Text {
                    text: "done".into(),
                }],
                Stop::EndTurn,
                1,
            )
        });
        with_hooks(
            &mut a,
            hook(
                "Stop",
                "",
                r#"echo '{"decision":"block","reason":"again"}'"#,
            ),
        );
        a.prompt("go", &mut Log::default()).unwrap();
        assert_eq!(*count.lock().unwrap(), 1 + MAX_STOP_CONTINUATIONS);
    }

    #[test]
    fn prompt_and_session_hooks() {
        let mut a = agent(|_, _| panic!("a blocked prompt must not reach the model"));
        with_hooks(
            &mut a,
            hook("UserPromptSubmit", "", "echo secret-check >&2; exit 2"),
        );
        let mut log = Log::default();
        a.prompt("my password is hunter2", &mut log).unwrap();
        assert!(a.messages.is_empty());
        assert_eq!(log.events, ["prompt blocked by a hook: secret-check"]);

        let mut a = agent(|req, _| {
            assert!(req.messages[0].text().starts_with("branch: main"));
            reply(vec![Block::Text { text: "ok".into() }], Stop::EndTurn, 1)
        });
        with_hooks(
            &mut a,
            json!({"hooks": {
                "SessionStart": [{"hooks": [{"type": "command", "command": "echo 'branch: main'"}]}],
                "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "cat >/dev/null; echo 'it is Tuesday'"}]}]
            }}),
        );
        a.start("startup", &mut Log::default()).unwrap();
        a.prompt("hello", &mut Log::default()).unwrap();
        assert_eq!(a.messages[0].text(), "branch: mainhelloit is Tuesday");
    }

    fn reply(content: Vec<Block>, stop: Stop, input: u64) -> Result<Reply, String> {
        Ok(Reply {
            content,
            stop,
            usage: Usage {
                input,
                ..Usage::default()
            },
        })
    }

    fn bash(id: &str, command: &str) -> Vec<Block> {
        vec![Block::ToolCall {
            id: id.into(),
            name: "Bash".into(),
            input: json!({ "command": command }),
        }]
    }

    fn agent(
        complete: impl Fn(&Request, &mut dyn FnMut(Delta)) -> Result<Reply, String>
        + Send
        + Sync
        + 'static,
    ) -> Agent {
        let mut a = Agent::new(
            Model::parse("claude-test"),
            std::env::temp_dir(),
            "sys".into(),
        );
        a.complete = Arc::new(complete);
        a
    }

    #[test]
    fn loops_through_tool_calls_until_the_model_answers() {
        let turn = Mutex::new(0);
        let mut agent = agent(move |req, on| {
            let mut turn = turn.lock().unwrap();
            *turn += 1;
            if *turn == 1 {
                return reply(bash("t1", "echo 42"), Stop::ToolUse, 10);
            }
            let Message::User { content } = req.messages.last().unwrap() else {
                panic!("expected tool results")
            };
            assert_eq!(
                content[0],
                Block::ToolResult {
                    id: "t1".into(),
                    output: "42".into(),
                    is_error: false
                }
            );
            on(Delta::Text("done"));
            reply(
                vec![Block::Text {
                    text: "done".into(),
                }],
                Stop::EndTurn,
                20,
            )
        });
        let mut log = Log::default();
        agent.prompt("go", &mut log).unwrap();
        assert_eq!(
            log.events,
            ["tool Bash", "ok true", "text done", "stop EndTurn"]
        );
        assert_eq!(agent.messages.len(), 4);
        assert_eq!(agent.context, 20);
    }

    #[test]
    fn stopping_during_a_reply_keeps_a_valid_history() {
        let mut agent = agent(|req, _| {
            while !req.cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(ABORTED.into())
        });
        let mut log = Log {
            stop_after_polls: Some(3),
            ..Log::default()
        };
        assert_eq!(agent.prompt("one", &mut log).unwrap_err(), ABORTED);
        assert_eq!(agent.messages.len(), 1);
        assert_eq!(
            agent.messages[0].text(),
            format!("one{}", compact::INTERRUPTED),
            "the model is told the turn was stopped"
        );
        // The next prompt folds into the unanswered one instead of adding a
        // second consecutive user message.
        agent.complete =
            Arc::new(|_, _| reply(vec![Block::Text { text: "hi".into() }], Stop::EndTurn, 5));
        agent.prompt("two", &mut Log::default()).unwrap();
        assert_eq!(agent.messages.len(), 2);
        assert_eq!(
            agent.messages[0].text(),
            format!("one{}two", compact::INTERRUPTED)
        );
    }

    #[test]
    fn stopping_during_a_command_answers_every_call() {
        let mut agent = agent(|_, _| {
            let mut calls = bash("a", "sleep 5");
            calls.extend(bash("b", "echo never"));
            reply(calls, Stop::ToolUse, 10)
        });
        let mut log = Log {
            stop_after_polls: Some(8),
            ..Log::default()
        };
        assert_eq!(agent.prompt("go", &mut log).unwrap_err(), ABORTED);
        let Message::User { content } = agent.messages.last().unwrap() else {
            panic!()
        };
        assert_eq!(content.len(), 3);
        assert_eq!(
            content[2],
            Block::Text {
                text: compact::INTERRUPTED.into()
            }
        );
        assert!(
            matches!(&content[0], Block::ToolResult { output, is_error: true, .. } if output.ends_with(tools::STOPPED))
        );
        assert!(
            matches!(&content[1], Block::ToolResult { output, .. } if output.starts_with("not run"))
        );
    }

    #[test]
    fn compacts_before_the_prompt_once_the_window_fills() {
        let summarized = Arc::new(Mutex::new(false));
        let seen = summarized.clone();
        let mut agent = agent(move |req, _| {
            if req.system == SUMMARY_SYSTEM {
                assert!(req.tools.is_empty());
                assert!(req.messages[0].text().contains("User: old"));
                *seen.lock().unwrap() = true;
                return reply(
                    vec![Block::Text {
                        text: "<summary>old work</summary>".into(),
                    }],
                    Stop::EndTurn,
                    1,
                );
            }
            reply(
                vec![Block::Text {
                    text: "x".repeat(200_000),
                }],
                Stop::EndTurn,
                950_000,
            )
        });
        agent.prompt("old", &mut Log::default()).unwrap();
        assert!(!*summarized.lock().unwrap());
        let mut log = Log::default();
        agent.prompt("recent", &mut log).unwrap();
        assert!(*summarized.lock().unwrap());
        assert!(log.events[0].starts_with("compacting"));
        assert_eq!(
            agent.messages.len(),
            2,
            "only the new reply follows the summary"
        );
        let first = agent.messages[0].text();
        assert!(
            first.starts_with("old") && first.contains("old work") && first.ends_with("recent")
        );
    }
}
