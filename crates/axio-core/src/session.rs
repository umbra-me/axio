//! The durable transcript, and the single choke point where it becomes wire
//! shape.
//!
//! In this milestone the transcript is in memory. The JSONL record format,
//! resume, and the truncated-tail repair land with persistence; the shape here
//! is already the one those will serialise, so adding them is additive.

use std::path::PathBuf;

use crate::protocol::{Item, ItemBody, SessionId, ToolStatus};
use crate::provider::{Role, WireContent, WireMessage};

/// A tool call and its result, as the loop hands them back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub struct Session {
    id: SessionId,
    cwd: PathBuf,
    model: String,
    transcript: Vec<Item>,
    files_changed: Vec<PathBuf>,
}

impl Session {
    pub fn new(cwd: PathBuf, model: impl Into<String>) -> Self {
        Self {
            id: SessionId::generate(),
            cwd,
            model: model.into(),
            transcript: Vec::new(),
            files_changed: Vec::new(),
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn transcript(&self) -> &[Item] {
        &self.transcript
    }

    pub fn push(&mut self, body: ItemBody) -> Item {
        let item = Item::new(body);
        self.transcript.push(item.clone());
        item
    }

    pub fn push_item(&mut self, item: Item) {
        self.transcript.push(item);
    }

    pub fn push_user(&mut self, text: &str) -> Item {
        self.push(ItemBody::UserMessage {
            text: text.to_owned(),
        })
    }

    /// Update a call in place by its id.
    ///
    /// The loop records a call as `Pending` when the model emits it, then
    /// resolves it here. Pushing a second item instead would give the call two
    /// `tool_result`s on the wire, which is invalid.
    pub fn set_tool_status(&mut self, call_id: &str, status: ToolStatus) -> Option<Item> {
        for item in self.transcript.iter_mut().rev() {
            if let ItemBody::ToolCall {
                call_id: id,
                status: existing,
                ..
            } = &mut item.body
                && id == call_id
            {
                *existing = status;
                return Some(item.clone());
            }
        }
        None
    }

    pub fn record_file_changed(&mut self, path: PathBuf) {
        if !self.files_changed.contains(&path) {
            self.files_changed.push(path);
        }
    }

    pub fn take_files_changed(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.files_changed)
    }

    /// The single place the durable transcript becomes wire shape.
    ///
    /// Everything provider-specific and everything display-only is decided here
    /// and nowhere else. Two rules are load-bearing:
    ///
    /// * Reasoning blocks are echoed back **verbatim**, signature included, and
    ///   only when the request model matches the one that minted them — another
    ///   model rejects or silently drops them.
    /// * Every `tool_use` is followed by its `tool_result`, and consecutive
    ///   results are merged into **one** user message in call order. Splitting
    ///   them across messages teaches the model to stop calling in parallel.
    pub fn wire_messages(&self, model: &str) -> Vec<WireMessage> {
        let same_model = model == self.model;
        let mut out: Vec<WireMessage> = Vec::new();

        let mut i = 0;
        while i < self.transcript.len() {
            // A run of consecutive tool calls is one step of the loop, and has
            // to become exactly two messages: one assistant message holding
            // every tool_use, then one user message holding every tool_result in
            // the same order. Emitting them interleaved — use, result, use,
            // result — is accepted by the API but teaches the model to stop
            // calling tools in parallel.
            if matches!(self.transcript[i].body, ItemBody::ToolCall { .. }) {
                let start = i;
                while i < self.transcript.len()
                    && matches!(self.transcript[i].body, ItemBody::ToolCall { .. })
                {
                    i += 1;
                }
                let run = &self.transcript[start..i];

                for item in run {
                    let ItemBody::ToolCall {
                        call_id,
                        name,
                        input,
                        ..
                    } = &item.body
                    else {
                        unreachable!("the run was selected by this variant")
                    };
                    push_content(
                        &mut out,
                        Role::Assistant,
                        WireContent::ToolUse {
                            id: call_id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        },
                    );
                }
                for item in run {
                    let ItemBody::ToolCall {
                        call_id, status, ..
                    } = &item.body
                    else {
                        unreachable!("the run was selected by this variant")
                    };
                    let (content, is_error) = result_of(status);
                    push_content(
                        &mut out,
                        Role::User,
                        WireContent::ToolResult {
                            tool_use_id: call_id.clone(),
                            content,
                            is_error,
                        },
                    );
                }
                continue;
            }

            let item = &self.transcript[i];
            i += 1;
            match &item.body {
                ItemBody::UserMessage { text } => push_content(
                    &mut out,
                    Role::User,
                    WireContent::Text { text: text.clone() },
                ),
                ItemBody::AgentMessage { text } => push_content(
                    &mut out,
                    Role::Assistant,
                    WireContent::Text { text: text.clone() },
                ),
                ItemBody::Reasoning { text, signature } => {
                    if same_model && !signature.is_empty() {
                        push_content(
                            &mut out,
                            Role::Assistant,
                            WireContent::Thinking {
                                thinking: text.clone(),
                                signature: signature.clone(),
                            },
                        );
                    }
                }
                ItemBody::ToolCall { .. } => unreachable!("handled as a run above"),
                ItemBody::Interrupted { after_steps } => push_content(
                    &mut out,
                    Role::User,
                    WireContent::Text {
                        text: format!(
                            "[the previous turn was interrupted by the user after {after_steps} step(s); \
                             work stopped there and was not completed]"
                        ),
                    },
                ),
                ItemBody::ContextElision { dropped_items } => push_content(
                    &mut out,
                    Role::User,
                    WireContent::Text {
                        text: format!(
                            "[{dropped_items} earlier item(s) elided to fit the context window]"
                        ),
                    },
                ),
            }
        }

        out
    }
}

/// A call with no terminal status yet is an orphan. A `tool_use` with no
/// matching `tool_result` is rejected outright, so one is synthesised rather
/// than the call being omitted.
fn result_of(status: &ToolStatus) -> (String, bool) {
    match status {
        ToolStatus::Ok { output, .. } => (output.clone(), false),
        ToolStatus::Failed { message } => (message.clone(), true),
        ToolStatus::Denied { message } => (message.clone(), true),
        ToolStatus::Cancelled => ("cancelled".to_owned(), true),
        ToolStatus::Pending | ToolStatus::AwaitingApproval | ToolStatus::Running => {
            ("interrupted before completion".to_owned(), true)
        }
    }
}

/// Append content, merging into the previous message when the role matches.
///
/// This merging is what puts every `tool_result` of one step into a single user
/// message.
fn push_content(out: &mut Vec<WireMessage>, role: Role, content: WireContent) {
    match out.last_mut() {
        Some(last) if last.role == role => last.content.push(content),
        _ => out.push(WireMessage {
            role,
            content: vec![content],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session() -> Session {
        Session::new(PathBuf::from("/w"), "claude-opus-5")
    }

    #[test]
    fn a_plain_exchange_alternates_roles() {
        let mut s = session();
        s.push_user("hi");
        s.push(ItemBody::AgentMessage {
            text: "hello".into(),
        });
        let wire = s.wire_messages("claude-opus-5");
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0].role, Role::User);
        assert_eq!(wire[1].role, Role::Assistant);
    }

    #[test]
    fn every_tool_result_of_a_step_lands_in_one_user_message_in_call_order() {
        let mut s = session();
        s.push_user("read two files");
        for (id, out) in [("toolu_1", "a"), ("toolu_2", "b"), ("toolu_3", "c")] {
            s.push(ItemBody::ToolCall {
                call_id: id.into(),
                name: "read".into(),
                input: json!({}),
                subject: "read:x".into(),
                preview: None,
                status: ToolStatus::Ok {
                    output: out.into(),
                    truncated: false,
                    spill: None,
                    ms: 1,
                },
            });
        }
        let wire = s.wire_messages("claude-opus-5");
        // user prompt, assistant with 3 tool_use, user with 3 tool_result
        assert_eq!(wire.len(), 3);
        assert_eq!(wire[1].role, Role::Assistant);
        assert_eq!(wire[1].content.len(), 3);
        assert_eq!(wire[2].role, Role::User);
        assert_eq!(wire[2].content.len(), 3, "results must not be split");
        let ids: Vec<&str> = wire[2]
            .content
            .iter()
            .map(|c| match c {
                WireContent::ToolResult { tool_use_id, .. } => tool_use_id.as_str(),
                _ => panic!("expected a tool_result"),
            })
            .collect();
        assert_eq!(ids, ["toolu_1", "toolu_2", "toolu_3"], "call order");
    }

    #[test]
    fn an_unfinished_tool_call_still_gets_a_result() {
        // A tool_use with no matching tool_result is rejected outright, so an
        // interrupted call must synthesise one rather than be omitted.
        let mut s = session();
        s.push(ItemBody::ToolCall {
            call_id: "toolu_9".into(),
            name: "bash".into(),
            input: json!({}),
            subject: "bash:git".into(),
            preview: None,
            status: ToolStatus::Running,
        });
        let wire = s.wire_messages("claude-opus-5");
        let last = wire.last().unwrap();
        assert_eq!(last.role, Role::User);
        assert!(matches!(
            &last.content[0],
            WireContent::ToolResult { is_error: true, .. }
        ));
    }

    #[test]
    fn reasoning_is_echoed_verbatim_with_its_signature() {
        let mut s = session();
        s.push(ItemBody::Reasoning {
            text: "summary".into(),
            signature: "sig123".into(),
        });
        let wire = s.wire_messages("claude-opus-5");
        assert_eq!(
            wire[0].content[0],
            WireContent::Thinking {
                thinking: "summary".into(),
                signature: "sig123".into()
            }
        );
    }

    #[test]
    fn reasoning_is_dropped_for_a_different_model() {
        // Another model rejects or silently drops a foreign thinking block, so
        // it must not be replayed across a model change.
        let mut s = session();
        s.push(ItemBody::Reasoning {
            text: "summary".into(),
            signature: "sig123".into(),
        });
        assert!(s.wire_messages("some-other-model").is_empty());
    }

    #[test]
    fn reasoning_without_a_signature_is_not_replayed() {
        let mut s = session();
        s.push(ItemBody::Reasoning {
            text: "summary".into(),
            signature: String::new(),
        });
        assert!(s.wire_messages("claude-opus-5").is_empty());
    }

    #[test]
    fn an_interrupt_tells_the_model_its_work_was_cut_short() {
        let mut s = session();
        s.push_user("do a big thing");
        s.push(ItemBody::Interrupted { after_steps: 3 });
        let wire = s.wire_messages("claude-opus-5");
        let text = match &wire.last().unwrap().content.last().unwrap() {
            WireContent::Text { text } => text.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(text.contains("interrupted"));
        assert!(text.contains('3'));
    }
}
