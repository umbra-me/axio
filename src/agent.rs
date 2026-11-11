use crate::compact;
use crate::message::{Block, Message};
use crate::provider::{self, Delta, Model, Reply, Request, Stop, Usage};
use crate::session::Session;
use crate::tools::{self, ToolSpec};
use serde_json::Value;
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
}

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
            tools: tools::specs(),
            complete: Arc::new(provider::complete),
        }
    }

    /// Compaction starts at 85% of the window, or AXIO_COMPACT_AT tokens.
    pub fn compact_at(&self) -> u64 {
        std::env::var("AXIO_COMPACT_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.model.context_window() / 100 * 85)
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

    /// Replaces the history with recent requests and a summary. Returns
    /// whether there was anything to compact.
    pub fn compact(&mut self, ui: &mut dyn Ui) -> Result<bool, String> {
        if !compact::worthwhile(&self.messages) {
            return Ok(false);
        }
        ui.notice("compacting the conversation…");
        let prompt = format!(
            "<transcript>\n{}</transcript>\n\n{}",
            compact::transcript(&self.messages),
            compact::INSTRUCTION
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
        if self.context >= self.compact_at() {
            match self.compact(ui) {
                Err(e) if e == ABORTED => return Err(e),
                Err(e) => ui.notice(&format!("compaction failed: {e}")),
                Ok(_) => {}
            }
        }
        self.push(Message::user_text(text))?;
        let result = self.run_turn(ui);
        if matches!(&result, Err(e) if e == ABORTED) {
            self.push(Message::user_text(compact::INTERRUPTED))?;
        }
        result
    }

    fn run_turn(&mut self, ui: &mut dyn Ui) -> Result<(), String> {
        loop {
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
                ui.stopped(&stop);
                return Ok(());
            }
            let mut results = Vec::new();
            let mut stopped = false;
            for (id, name, input) in calls {
                stopped = stopped || ui.interrupted();
                // A reply cut off at max_tokens can end inside a tool call.
                let result = if stopped {
                    Err("not run: the user stopped the turn".into())
                } else if stop == Stop::MaxTokens {
                    Err("the reply hit the output limit before this call was complete".into())
                } else {
                    ui.tool_start(&name, &input);
                    let r = tools::run(&name, &input, &self.cwd, &mut || ui.interrupted());
                    ui.tool_end(&r);
                    stopped = matches!(&r, Err(e) if e.ends_with(tools::STOPPED));
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
