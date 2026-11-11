use crate::message::{Block, Message};
use crate::provider::{self, Delta, Model, Reply, Request, Stop, Usage};
use crate::session::Session;
use crate::tools::{self, ToolSpec};
use serde_json::Value;
use std::path::PathBuf;

pub trait Ui {
    fn delta(&mut self, d: Delta);
    fn tool_start(&mut self, name: &str, input: &Value);
    fn tool_end(&mut self, result: &Result<String, String>);
    fn stopped(&mut self, stop: &Stop);
    fn usage(&mut self, u: Usage);
}

pub type Complete = dyn FnMut(&Request, &mut dyn FnMut(Delta)) -> Result<Reply, String>;

pub struct Agent {
    pub model: Model,
    pub effort: Option<String>,
    pub cwd: PathBuf,
    pub system: String,
    pub messages: Vec<Message>,
    pub session: Option<Session>,
    tools: Vec<ToolSpec>,
    complete: Box<Complete>,
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
            tools: tools::specs(),
            complete: Box::new(provider::complete),
        }
    }

    fn push(&mut self, m: Message) -> Result<(), String> {
        if let Some(s) = &mut self.session {
            s.append(&m)?;
        }
        self.messages.push(m);
        Ok(())
    }

    /// Runs one user prompt to completion: model replies and tool calls repeat
    /// until the model answers without calling a tool.
    pub fn prompt(&mut self, text: &str, ui: &mut dyn Ui) -> Result<(), String> {
        self.push(Message::user_text(text))?;
        loop {
            let reply = {
                let req = Request {
                    model: &self.model,
                    system: &self.system,
                    messages: &self.messages,
                    tools: &self.tools,
                    effort: self.effort.as_deref(),
                };
                (self.complete)(&req, &mut |d| ui.delta(d))?
            };
            let (msg, stop, usage) = reply.into_message();
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
            for (id, name, input) in calls {
                ui.tool_start(&name, &input);
                // A reply cut off at max_tokens can end inside a tool call.
                let result = if stop == Stop::MaxTokens {
                    Err("the reply hit the output limit before this call was complete".into())
                } else {
                    tools::run(&name, &input, &self.cwd)
                };
                ui.tool_end(&result);
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct Log(Vec<String>);
    impl Ui for Log {
        fn delta(&mut self, d: Delta) {
            if let Delta::Text(t) = d {
                self.0.push(format!("text {t}"));
            }
        }
        fn tool_start(&mut self, name: &str, _: &Value) {
            self.0.push(format!("tool {name}"));
        }
        fn tool_end(&mut self, r: &Result<String, String>) {
            self.0.push(format!("ok {}", r.is_ok()));
        }
        fn stopped(&mut self, s: &Stop) {
            self.0.push(format!("stop {s:?}"));
        }
        fn usage(&mut self, _: Usage) {}
    }

    #[test]
    fn loops_through_tool_calls_until_the_model_answers() {
        let mut turn = 0;
        let mut agent = Agent::new(
            Model::parse("claude-test"),
            std::env::temp_dir(),
            "sys".into(),
        );
        agent.complete = Box::new(move |req, on| {
            turn += 1;
            if turn == 1 {
                return Ok(Reply {
                    content: vec![Block::ToolCall {
                        id: "t1".into(),
                        name: "bash".into(),
                        input: json!({"command": "echo 42"}),
                    }],
                    stop: Stop::ToolUse,
                    usage: Usage::default(),
                });
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
            Ok(Reply {
                content: vec![Block::Text {
                    text: "done".into(),
                }],
                stop: Stop::EndTurn,
                usage: Usage::default(),
            })
        });
        let mut log = Log::default();
        agent.prompt("go", &mut log).unwrap();
        assert_eq!(log.0, ["tool bash", "ok true", "text done", "stop EndTurn"]);
        assert_eq!(agent.messages.len(), 4);
    }
}
