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
    /// Messages the user sent while the turn ran, for the model now.
    fn steered(&mut self) -> Vec<String> {
        Vec::new()
    }
    /// A yes/no question for the user; without a terminal the answer is no.
    fn ask(&mut self, _question: &str) -> bool {
        false
    }
    /// Sub-agents are running: one line on what they are doing.
    fn agents_running(&mut self, _line: &str) {}
    /// A sub-agent finished; `summary` is its tool calls and time.
    fn agent_done(&mut self, _label: &str, _result: &Result<String, String>, _summary: &str) {}
}

/// Sent with the last tool results a turn limit allows.
pub const TURN_LIMIT_NOTE: &str = "[axio] You have reached your turn limit. Do not call any more tools; reply now with your final answer from what you have found.";

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
    /// Retries of a model call that failed for a passing reason, and the
    /// first wait between them (it doubles).
    pub max_retries: u32,
    pub retry_unit: Duration,
    /// Rounds of tool calls one prompt may take; after the last one the
    /// model gets one more reply to answer without tools.
    pub max_turns: Option<usize>,
    /// Files the latest turn wrote or edited, with what they held before
    /// (`None`: the turn created them), for /undo.
    changed: std::sync::Mutex<Vec<(PathBuf, Option<Vec<u8>>)>>,
    mcp: Arc<Mcp>,
    subagents: Option<Arc<crate::subagent::Host>>,
    tools: Vec<ToolSpec>,
    complete: Arc<Complete>,
}

enum Event {
    Text(String),
    Thinking,
    ThinkingText(String),
    ToolCall(String),
    ToolInput(String),
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
            subagents: None,
            tools: tools::specs(),
            complete: Arc::new(provider::complete),
            max_retries: 4,
            retry_unit: Duration::from_secs(1),
            max_turns: None,
            changed: Default::default(),
        }
    }

    /// The core tools, the profile's modules and started MCP servers' tools.
    pub fn set_tools(&mut self, modules: Modules, mcp: Mcp) {
        self.modules = modules;
        self.tools = tools::specs_for(modules);
        self.tools.extend(mcp.specs.iter().cloned());
        self.mcp = Arc::new(mcp);
    }

    /// Adds the Agent tool, run by `host`.
    pub fn set_subagents(&mut self, host: crate::subagent::Host) {
        self.tools.push(host.tool_spec());
        self.subagents = Some(Arc::new(host));
    }

    #[cfg(test)]
    pub fn set_complete(
        &mut self,
        f: impl Fn(&Request, &mut dyn FnMut(Delta)) -> Result<Reply, String> + Send + Sync + 'static,
    ) {
        self.complete = Arc::new(f);
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name.as_str()).collect()
    }

    /// Offers only the named tools. Calls to any other tool are refused.
    pub fn keep_tools(&mut self, names: &[String]) -> Result<(), String> {
        if let Some(missing) = names
            .iter()
            .find(|n| !self.tools.iter().any(|t| &t.name == *n))
        {
            return Err(format!("no tool named {missing} here"));
        }
        self.tools.retain(|t| names.contains(&t.name));
        Ok(())
    }

    /// Compaction starts at 85% of the window, or AXIO_COMPACT_AT tokens.
    /// A rough size of the next request, for when no reply has measured it:
    /// the history plus the system prompt and tool definitions.
    pub fn estimated_context(&self) -> u64 {
        let tools: usize = self
            .tools
            .iter()
            .map(|t| t.name.len() + t.description.len() + t.schema.to_string().len())
            .sum();
        compact::estimate_tokens(&self.messages) + (self.system.len() + tools) as u64 / 4
    }

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

    /// One model call, retried with backoff when it fails for a passing
    /// reason before any of its text was shown.
    fn call(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Vec<ToolSpec>,
        stream: bool,
        ui: &mut dyn Ui,
    ) -> Result<Reply, String> {
        let mut attempt = 0;
        loop {
            let mut shown = false;
            let err = match self.call_once(
                system,
                messages.clone(),
                tools.clone(),
                stream,
                ui,
                &mut shown,
            ) {
                Err(e) if e != ABORTED => e,
                result => return result,
            };
            let reason = provider::retry_reason(&err).filter(|_| !shown);
            let Some(reason) = reason.filter(|_| attempt < self.max_retries) else {
                return Err(match (reason, attempt) {
                    (Some(_), n) if n > 0 => format!("{err} (gave up after {n} retries)"),
                    _ => err,
                });
            };
            let wait = provider::retry_wait(&err, attempt, self.retry_unit);
            attempt += 1;
            ui.notice(&format!(
                "{reason}; retrying in {}s ({attempt} of {})",
                wait.as_secs().max(1),
                self.max_retries
            ));
            ui.waiting(&format!("{reason}; waiting to retry"));
            let until = std::time::Instant::now() + wait;
            while std::time::Instant::now() < until {
                if ui.interrupted() {
                    return Err(ABORTED.into());
                }
                std::thread::sleep(Duration::from_millis(20).min(wait));
            }
        }
    }

    /// Runs one model call on a worker thread so the user can stop it at once;
    /// the stream sees the cancel flag and closes the connection.
    fn call_once(
        &self,
        system: &str,
        messages: Vec<Message>,
        tools: Vec<ToolSpec>,
        stream: bool,
        ui: &mut dyn Ui,
        shown: &mut bool,
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
                    Delta::ThinkingText(t) => Event::ThinkingText(t.to_string()),
                    Delta::ToolCall(n) => Event::ToolCall(n.to_string()),
                    Delta::ToolInput(p) => Event::ToolInput(p.to_string()),
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
                Ok(Event::Text(t)) => {
                    *shown = true;
                    ui.delta(Delta::Text(&t));
                }
                Ok(Event::Thinking) => ui.delta(Delta::Thinking),
                Ok(Event::ThinkingText(t)) => ui.delta(Delta::ThinkingText(&t)),
                Ok(Event::ToolCall(n)) => ui.delta(Delta::ToolCall(&n)),
                Ok(Event::ToolInput(p)) => ui.delta(Delta::ToolInput(&p)),
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
            // Labelled so it is never mistaken for something the user typed.
            self.push(Message::user_text(format!(
                "[Context from a SessionStart hook:]\n{c}"
            )))?;
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
        let before = self.context.max(self.estimated_context());
        compact::apply(&mut self.messages, &summary);
        if let Some(s) = &mut self.session {
            s.compaction(&summary)?;
        }
        let after = self.estimated_context();
        ui.notice(&format!(
            "compacted: {} → about {} tokens",
            crate::ui::tokens(before),
            crate::ui::tokens(after)
        ));
        // An estimate until the next reply reports the real size.
        self.context = after;
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
        if let Ok(mut changed) = self.changed.lock() {
            changed.clear();
        }
        let mut content = vec![Block::Text { text: text.into() }];
        for (block, note) in crate::context::mentions(text, &self.cwd) {
            ui.notice(&note);
            content.push(block);
        }
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
        let mut rounds = 0;
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
            rounds += 1;
            let over = self.max_turns.filter(|max| rounds > *max);
            if let (Some(max), false) = (over, calls.is_empty()) {
                let content = calls
                    .into_iter()
                    .map(|(id, _, _)| Block::ToolResult {
                        id,
                        output: "not run: the turn limit was reached".into(),
                        is_error: true,
                    })
                    .collect();
                self.push(Message::User { content })?;
                return Err(format!(
                    "reached the turn limit ({max}) without a final answer"
                ));
            }
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
            let mut images = Vec::new();
            // Agent calls start together after the loop: (result slot, id,
            // input, context from PreToolUse, plan).
            let mut pending: Vec<(usize, String, Value, Vec<String>, crate::subagent::Planned)> =
                Vec::new();
            for (id, name, input) in calls {
                stopped = stopped || ui.interrupted();
                let mut input_path = None;
                // A reply cut off at max_tokens can end inside a tool call.
                let result = if stopped {
                    Err("not run: the user stopped the turn".into())
                } else if let Some(reason) = &hook_stop {
                    Err(format!("not run: a hook ended the turn ({reason})"))
                } else if !self.tools.iter().any(|t| t.name == name) {
                    Err(format!("no tool named {name} here"))
                } else if stop == Stop::MaxTokens {
                    Err("the reply hit the output limit before this call was complete".into())
                } else if name == "Agent"
                    && let Some(host) = self.subagents.clone()
                {
                    match self.plan_agent(&id, input, &host, ui) {
                        Ok((input, context, plan)) => {
                            pending.push((results.len(), id.clone(), input, context, plan));
                            Ok(String::new())
                        }
                        Err((r, end)) => {
                            hook_stop = end;
                            r
                        }
                    }
                } else {
                    // The path as run, after any hook rewrote the input.
                    let (r, end, ran) = self.run_tool(&id, &name, input, ui);
                    input_path = ran["file_path"].as_str().map(String::from);
                    stopped = matches!(&r, Err(e) if e.ends_with(tools::STOPPED));
                    hook_stop = end;
                    r
                };
                let (output, is_error) = match result {
                    Ok(o) => (o, false),
                    Err(e) => (e, true),
                };
                if name == "Read" && !is_error && output.starts_with("[image") {
                    let path = input_path.as_deref().unwrap_or_default();
                    match crate::context::image_block(&tools::resolve(&self.cwd, path)) {
                        Ok((block, _)) => images.push(block),
                        Err(e) => images.push(Block::Text {
                            text: format!("[{path}: {e}]"),
                        }),
                    }
                }
                results.push(Block::ToolResult {
                    id,
                    output,
                    is_error,
                });
            }
            if !pending.is_empty() {
                let host = self.subagents.clone().expect("pending agents have a host");
                let plans: Vec<_> = pending.iter().map(|p| &p.4).collect();
                let outs = match (&hook_stop, stopped) {
                    (Some(reason), _) => plans
                        .iter()
                        .map(|_| Err(format!("not run: a hook ended the turn ({reason})")))
                        .collect(),
                    (None, true) => plans
                        .iter()
                        .map(|_| Err(format!("not run: {}", tools::STOPPED)))
                        .collect(),
                    (None, false) => host.run_all(&plans, ui),
                };
                for ((slot, id, input, context, _), out) in pending.into_iter().zip(outs) {
                    stopped = stopped || matches!(&out, Err(e) if e.ends_with(tools::STOPPED));
                    let (out, end) = self.post_tool(&id, "Agent", &input, out, context, ui);
                    hook_stop = hook_stop.or(end);
                    let (output, is_error) = match out {
                        Ok(o) => (o, false),
                        Err(e) => (e, true),
                    };
                    results[slot] = Block::ToolResult {
                        id,
                        output,
                        is_error,
                    };
                }
            }
            // Images a Read returned follow the results they belong to.
            results.append(&mut images);
            if self.max_turns == Some(rounds) {
                results.push(Block::Text {
                    text: TURN_LIMIT_NOTE.into(),
                });
            }
            for text in ui.steered() {
                results.push(Block::Text {
                    text: format!("[The user wrote this while you were working:]\n{text}"),
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
    /// Keeps a file's contents from before this turn first changed it.
    fn remember_original(&self, input: &Value) {
        let Some(path) = input["file_path"]
            .as_str()
            .map(|p| tools::resolve(&self.cwd, p))
        else {
            return;
        };
        if let Ok(mut changed) = self.changed.lock()
            && !changed.iter().any(|(p, _)| *p == path)
        {
            let before = std::fs::read(&path).ok();
            changed.push((path, before));
        }
    }

    /// Puts back the files the latest turn wrote or edited, and tells the
    /// model. Returns what was restored (`true`) or removed (`false`).
    pub fn undo(&mut self) -> Result<Vec<(PathBuf, bool)>, String> {
        let changed = std::mem::take(&mut *self.changed.lock().map_err(|e| e.to_string())?);
        if changed.is_empty() {
            return Err("nothing to undo: the last turn wrote or edited no files".into());
        }
        // Try every file; keep the ones that failed so /undo can be retried.
        let (mut done, mut failed, mut errors) = (Vec::new(), Vec::new(), Vec::new());
        for (path, before) in changed.into_iter().rev() {
            let result = match &before {
                Some(bytes) => std::fs::write(&path, bytes),
                None => std::fs::remove_file(&path).or_else(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(e)
                    }
                }),
            };
            match result {
                Ok(()) => done.push((path, before.is_some())),
                Err(e) => {
                    errors.push(format!("{}: {e}", path.display()));
                    failed.push((path, before));
                }
            }
        }
        if !done.is_empty() {
            let names: Vec<String> = done.iter().map(|(p, _)| p.display().to_string()).collect();
            let mut note = format!(
                "[The user undid the file changes from your previous turn: {}. Commands you ran were not undone.",
                names.join(", ")
            );
            if !errors.is_empty() {
                note.push_str(&format!(
                    " These could not be put back: {}.",
                    errors.join("; ")
                ));
            }
            note.push(']');
            self.push(Message::user_text(note))?;
        }
        if !failed.is_empty() {
            failed.reverse();
            if let Ok(mut changed) = self.changed.lock() {
                *changed = failed;
            }
            return Err(format!(
                "restored {} file(s); could not put back: {} (run /undo again once fixed)",
                done.len(),
                errors.join("; ")
            ));
        }
        Ok(done)
    }

    /// PreToolUse for one call: the input to run (a hook may rewrite it), a
    /// refusal if the call must not run, and context hooks added. `Err` when
    /// a hook ended the turn.
    #[allow(clippy::type_complexity)]
    fn pre_tool(
        &self,
        id: &str,
        name: &str,
        mut input: Value,
        ui: &mut dyn Ui,
    ) -> Result<(Value, Option<String>, Vec<String>), (Result<String, String>, Option<String>, Value)>
    {
        let fields = json!({"tool_name": name, "tool_input": input, "tool_use_id": id});
        let pre = self.hook(Hook::PreToolUse, Some(name), fields, ui);
        if let Some(reason) = pre.stop {
            return Err((
                Err(format!("not run: a hook ended the turn ({reason})")),
                Some(reason),
                input,
            ));
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
                    .or(input["description"].as_str())
                    .unwrap_or("");
                (!ui.ask(&format!("Allow {name} {summary}{why}?")))
                    .then(|| format!("the user declined this call{why}"))
            }
            _ => None,
        };
        Ok((input, refusal, pre.context))
    }

    /// PostToolUse for one call: the result with anything hooks added, and
    /// the reason when a hook ended the turn.
    fn post_tool(
        &self,
        id: &str,
        name: &str,
        input: &Value,
        mut result: Result<String, String>,
        mut extra: Vec<String>,
        ui: &mut dyn Ui,
    ) -> (Result<String, String>, Option<String>) {
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

    fn run_tool(
        &self,
        id: &str,
        name: &str,
        input: Value,
        ui: &mut dyn Ui,
    ) -> (Result<String, String>, Option<String>, Value) {
        let (input, refusal, context) = match self.pre_tool(id, name, input, ui) {
            Ok(x) => x,
            Err(done) => return done,
        };
        let result = match refusal {
            Some(r) => {
                // Show refused calls too, so the user sees what was asked.
                ui.tool_start(name, &input);
                let r = Err(r);
                ui.tool_end(&r);
                r
            }
            None => {
                ui.tool_start(name, &input);
                if matches!(name, "Write" | "Edit") && !self.mcp.owns(name) {
                    self.remember_original(&input);
                }
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
        let (result, stop) = self.post_tool(id, name, &input, result, context, ui);
        (result, stop, input)
    }

    /// PreToolUse and planning for an Agent call. `Err` holds the finished
    /// result (refused, invalid, or the turn ended) and any hook stop.
    #[allow(clippy::type_complexity)]
    fn plan_agent(
        &self,
        id: &str,
        input: Value,
        host: &crate::subagent::Host,
        ui: &mut dyn Ui,
    ) -> Result<
        (Value, Vec<String>, crate::subagent::Planned),
        (Result<String, String>, Option<String>),
    > {
        let (input, refusal, context) = match self.pre_tool(id, "Agent", input, ui) {
            Ok(x) => x,
            Err((r, end, _)) => return Err((r, end)),
        };
        let names: Vec<String> = self
            .tools
            .iter()
            .filter(|t| t.name != "Agent")
            .map(|t| t.name.clone())
            .collect();
        let parent = crate::roles::Parent {
            model: &self.model,
            effort: self.effort.as_deref(),
            cwd: &self.cwd,
            tools: &names,
            sandboxed: crate::sandbox::active().is_some(),
            may_nest: host.children_may_nest(),
        };
        let planned = match refusal {
            Some(r) => Err(r),
            None => host.prepare(&input, &parent),
        };
        ui.tool_start("Agent", &input);
        match planned {
            Ok(mut plan) => {
                if plan.spec.context == crate::child::Sees::Fork {
                    // This reply, which holds the Agent call, is left out.
                    let before = &self.messages[..self.messages.len().saturating_sub(1)];
                    plan.spec.history = Some(compact::transcript(before));
                }
                Ok((input, context, plan))
            }
            Err(e) => {
                let r = Err(e);
                ui.tool_end(&r);
                Err(self.post_tool(id, "Agent", &input, r, context, ui))
            }
        }
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
            let first = req.messages[0].text();
            assert!(first.starts_with('[') && first.contains("branch: main"));
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
        assert!(
            a.messages[0]
                .text()
                .ends_with("\nbranch: mainhelloit is Tuesday")
        );
        assert_eq!(crate::ui::recap(&a.messages)[0], "› hello");
    }

    #[test]
    fn messages_sent_mid_turn_reach_the_model_with_the_results() {
        struct Steer(Vec<String>, Log);
        impl Ui for Steer {
            fn tool_start(&mut self, n: &str, i: &Value) {
                self.1.tool_start(n, i)
            }
            fn delta(&mut self, _: Delta) {}
            fn tool_end(&mut self, _: &Result<String, String>) {}
            fn stopped(&mut self, _: &Stop) {}
            fn usage(&mut self, _: Usage) {}
            fn steered(&mut self) -> Vec<String> {
                std::mem::take(&mut self.0)
            }
        }
        let mut a = bash_then_echo("echo hi");
        let mut ui = Steer(vec!["use printf instead".into()], Log::default());
        a.prompt("go", &mut ui).unwrap();
        let Message::User { content } = &a.messages[2] else {
            panic!()
        };
        assert!(matches!(&content[0], Block::ToolResult { .. }));
        assert_eq!(
            content[1],
            Block::Text {
                text: "[The user wrote this while you were working:]\nuse printf instead".into()
            }
        );
    }

    #[test]
    fn reading_an_image_shows_it_to_the_model() {
        let dir = std::env::temp_dir().join(format!("axio-read-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shot.png"), b"\x89PNG").unwrap();
        let turn = Mutex::new(0);
        let mut agent = agent(move |_, _| {
            let mut t = turn.lock().unwrap();
            *t += 1;
            match *t {
                1 => reply(
                    vec![Block::ToolCall {
                        id: "1".into(),
                        name: "Read".into(),
                        input: json!({"file_path": "shot.png"}),
                    }],
                    Stop::ToolUse,
                    1,
                ),
                _ => reply(vec![Block::Text { text: "red".into() }], Stop::EndTurn, 1),
            }
        });
        agent.cwd = dir.clone();
        agent.prompt("look", &mut Log::default()).unwrap();
        let Message::User { content } = &agent.messages[2] else {
            panic!()
        };
        assert!(
            matches!(&content[0], Block::ToolResult { output, is_error: false, .. } if output.starts_with("[image, 1 KB"))
        );
        assert_eq!(
            content[1],
            Block::Image {
                media_type: "image/png".into(),
                data: "iVBORw==".into()
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_that_partly_fails_can_be_retried() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("axio-undo-partial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a, "a1").unwrap();
        std::fs::write(&b, "b1").unwrap();
        let (a2, b2) = (a.display().to_string(), b.display().to_string());
        let turn = Mutex::new(0);
        let mut agent = agent(move |_, _| {
            let mut t = turn.lock().unwrap();
            *t += 1;
            match *t {
                1 => reply(
                    vec![
                        Block::ToolCall {
                            id: "1".into(),
                            name: "Write".into(),
                            input: json!({"file_path": a2, "content": "a2"}),
                        },
                        Block::ToolCall {
                            id: "2".into(),
                            name: "Write".into(),
                            input: json!({"file_path": b2, "content": "b2"}),
                        },
                    ],
                    Stop::ToolUse,
                    1,
                ),
                _ => reply(
                    vec![Block::Text {
                        text: "done".into(),
                    }],
                    Stop::EndTurn,
                    1,
                ),
            }
        });
        agent.prompt("go", &mut Log::default()).unwrap();
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o444)).unwrap();
        let err = agent.undo().unwrap_err();
        assert!(err.contains("b.txt") && err.contains("restored 1"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&a).unwrap(),
            "a1",
            "the others are still restored"
        );
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            agent.undo().unwrap(),
            [(b.clone(), true)],
            "retry does the rest"
        );
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_restores_edited_files_and_removes_created_ones() {
        let dir = std::env::temp_dir().join(format!("axio-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        let (a2, b2) = (a.display().to_string(), b.display().to_string());
        let turn = Mutex::new(0);
        let mut agent = agent(move |_, _| {
            let mut t = turn.lock().unwrap();
            *t += 1;
            match *t {
                1 => reply(
                    vec![
                        Block::ToolCall {
                            id: "1".into(),
                            name: "Edit".into(),
                            input: json!({"file_path": a2, "old_string": "one", "new_string": "two"}),
                        },
                        Block::ToolCall {
                            id: "2".into(),
                            name: "Edit".into(),
                            input: json!({"file_path": a2, "old_string": "two", "new_string": "three"}),
                        },
                        Block::ToolCall {
                            id: "3".into(),
                            name: "Write".into(),
                            input: json!({"file_path": b2, "content": "new"}),
                        },
                    ],
                    Stop::ToolUse,
                    1,
                ),
                _ => reply(
                    vec![Block::Text {
                        text: "done".into(),
                    }],
                    Stop::EndTurn,
                    1,
                ),
            }
        });
        agent.cwd = dir.clone();
        agent.prompt("go", &mut Log::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "three\n");
        let done = agent.undo().unwrap();
        assert_eq!(done, [(b.clone(), false), (a.clone(), true)]);
        assert_eq!(
            std::fs::read_to_string(&a).unwrap(),
            "one\n",
            "back to before the turn"
        );
        assert!(!b.exists(), "created files are removed");
        assert!(
            agent
                .messages
                .last()
                .unwrap()
                .text()
                .contains("undid the file changes")
        );
        assert!(agent.undo().is_err(), "only once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retries_passing_failures_before_anything_is_shown() {
        let calls = Arc::new(Mutex::new(0));
        let n = calls.clone();
        let mut a = agent(move |_, on| {
            let mut n = n.lock().unwrap();
            *n += 1;
            match *n {
                1 => Err("529 from https://x: overloaded".into()),
                2 => Err("request to https://x failed: reset".into()),
                _ => {
                    on(Delta::Text("hi"));
                    reply(vec![Block::Text { text: "hi".into() }], Stop::EndTurn, 1)
                }
            }
        });
        a.retry_unit = Duration::from_millis(1);
        let mut log = Log::default();
        a.prompt("go", &mut log).unwrap();
        assert_eq!(*calls.lock().unwrap(), 3);
        assert_eq!(
            log.events[..2],
            [
                "the API is overloaded; retrying in 1s (1 of 4)",
                "the connection failed; retrying in 1s (2 of 4)"
            ]
        );

        // Not after text was shown, and never for a request the API rejected.
        for (shows, err) in [(true, "529 from u: busy"), (false, "400 from u: bad")] {
            let calls = Arc::new(Mutex::new(0));
            let n = calls.clone();
            let mut a = agent(move |_, on| {
                *n.lock().unwrap() += 1;
                if shows {
                    on(Delta::Text("partial"));
                }
                Err(err.into())
            });
            a.retry_unit = Duration::from_millis(1);
            assert_eq!(a.prompt("go", &mut Log::default()).unwrap_err(), err);
            assert_eq!(*calls.lock().unwrap(), 1);
        }

        let mut a = agent(|_, _| Err("529 from u: busy".into()));
        a.retry_unit = Duration::from_millis(1);
        a.max_retries = 2;
        assert_eq!(
            a.prompt("go", &mut Log::default()).unwrap_err(),
            "529 from u: busy (gave up after 2 retries)"
        );
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
    fn compaction_estimate_counts_the_system_prompt() {
        let mut agent = agent(|req, _| {
            let text = if req.system == SUMMARY_SYSTEM {
                "<summary>old work</summary>"
            } else {
                "ok"
            };
            reply(
                vec![Block::Text { text: text.into() }],
                Stop::EndTurn,
                12_000,
            )
        });
        agent.system = "s".repeat(40_000);
        agent.prompt("old", &mut Log::default()).unwrap();
        assert!(agent.compact("manual", &mut Log::default()).unwrap());
        assert!(
            agent.context >= 10_000,
            "the system prompt still counts after compacting, got {}",
            agent.context
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

    #[test]
    fn refuses_a_tool_it_was_not_given() {
        let marker = std::env::temp_dir().join(format!("axio-refused-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let command = format!("touch {}", marker.display());
        let turn = Mutex::new(0);
        let mut agent = agent(move |req, _| {
            assert_eq!(
                req.tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>(),
                ["Read"]
            );
            let mut turn = turn.lock().unwrap();
            *turn += 1;
            if *turn == 1 {
                return reply(bash("t1", &command), Stop::ToolUse, 1);
            }
            let Message::User { content } = req.messages.last().unwrap() else {
                panic!("expected tool results")
            };
            assert!(
                matches!(&content[0], Block::ToolResult { output, is_error: true, .. } if output.contains("no tool named Bash"))
            );
            reply(vec![Block::Text { text: "ok".into() }], Stop::EndTurn, 1)
        });
        agent.keep_tools(&["Read".into()]).unwrap();
        agent.prompt("go", &mut Log::default()).unwrap();
        assert!(!marker.exists(), "the refused command must not run");
        assert!(
            agent
                .keep_tools(&["Nope".into()])
                .unwrap_err()
                .contains("Nope")
        );
    }

    #[test]
    fn a_turn_limit_asks_for_an_answer_then_ends_the_turn() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut agent = agent(move |req, _| {
            let last = req.messages.last().unwrap().text();
            seen.lock().unwrap().push(last);
            reply(bash("t", "true"), Stop::ToolUse, 1)
        });
        agent.max_turns = Some(2);
        let err = agent.prompt("go", &mut Log::default()).unwrap_err();
        assert!(err.contains("turn limit (2)"), "{err}");
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            3,
            "two rounds of tools, then one for the answer"
        );
        assert!(!calls[1].contains(TURN_LIMIT_NOTE));
        assert!(calls[2].contains(TURN_LIMIT_NOTE));
        // The history stays one the provider accepts: every call answered.
        let Some(Message::User { content }) = agent.messages.last() else {
            panic!("the refused calls are answered")
        };
        assert!(matches!(
            &content[0],
            Block::ToolResult { is_error: true, .. }
        ));
    }

    #[test]
    fn queued_agent_calls_do_not_start_once_the_turn_ends() {
        let mut agent = agent(|req, _| {
            if req.messages.len() == 1 {
                let mut calls = vec![Block::ToolCall {
                    id: "a".into(),
                    name: "Agent".into(),
                    input: json!({"description": "d", "prompt": "p", "subagent_type": "explore"}),
                }];
                calls.extend(bash("b", "true"));
                return reply(calls, Stop::ToolUse, 1);
            }
            reply(vec![Block::Text { text: "ok".into() }], Stop::EndTurn, 1)
        });
        let (roles, _) = crate::roles::load(&[]);
        let host =
            crate::subagent::Host::new(roles, crate::settings::Subagents::default(), 0).unwrap();
        agent.set_subagents(host);
        with_hooks(
            &mut agent,
            hook(
                "PreToolUse",
                "Bash",
                r#"echo '{"continue": false, "stopReason": "halt"}'"#,
            ),
        );
        agent.prompt("go", &mut Log::default()).unwrap();
        let Some(Message::User { content }) = agent.messages.last() else {
            panic!("results")
        };
        assert!(
            matches!(&content[0], Block::ToolResult { id, output, is_error: true } if id == "a" && output.contains("not run")),
            "{content:?}"
        );
    }
}
