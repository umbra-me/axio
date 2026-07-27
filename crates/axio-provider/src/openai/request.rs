//! Building the request body for the chat-completions dialect.
//!
//! The shapes differ from the Messages dialect in ways that are not cosmetic: a
//! tool result is its own message with a role of its own, so one turn's results
//! become several messages rather than one.

use super::*;

/// Build the request body in the chat-completions dialect.
pub fn build_body(req: &ModelRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    // The system prompt is a message here rather than a top-level field.
    if !req.system.is_empty() {
        let text = req
            .system
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        messages.push(json!({ "role": "system", "content": text }));
    }
    messages.extend(wire_to_openai(&req.messages));

    let mut body = Map::new();
    body.insert("model".into(), json!(req.model));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("stream".into(), json!(true));
    // Without this the stream carries no usage at all, and a turn that cannot
    // report what it cost cannot enforce a budget either.
    body.insert("stream_options".into(), json!({ "include_usage": true }));
    body.insert("max_tokens".into(), json!(req.max_tokens));

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }

    Value::Object(body)
}

/// Convert the wire projection into this dialect's message list.
///
/// This is the seam the trait was uncertain about. One user message carrying
/// three `tool_result`s is correct for the Messages API and invalid here, where
/// each result is its own message with `role: "tool"`. Splitting is mechanical,
/// which is the useful finding: the shape difference is real but local to the
/// provider, and nothing above it had to change.
pub(super) fn wire_to_openai(messages: &[axio_core::provider::WireMessage]) -> Vec<Value> {
    let mut out = Vec::new();

    for message in messages {
        let mut text = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut results: Vec<Value> = Vec::new();

        for content in &message.content {
            match content {
                WireContent::Text { text: t } => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
                // No equivalent, and inventing one would be worse than
                // omitting it. The transcript keeps the block either way.
                WireContent::Thinking { .. } => {}
                WireContent::ToolUse { id, name, input } => tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        // Arguments are a JSON *string* here, not an object.
                        "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                    }
                })),
                WireContent::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => results.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                })),
            }
        }

        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        if !text.is_empty() || !tool_calls.is_empty() {
            let mut m = Map::new();
            m.insert("role".into(), json!(role));
            m.insert("content".into(), json!(text));
            if !tool_calls.is_empty() {
                m.insert("tool_calls".into(), Value::Array(tool_calls));
            }
            out.push(Value::Object(m));
        }
        // Each result becomes its own message, in order.
        out.extend(results);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_core::provider::{Role, WireContent, WireMessage};

    #[test]
    fn a_batch_of_tool_results_becomes_one_message_each() {
        let messages = vec![
            WireMessage {
                role: Role::Assistant,
                content: vec![
                    WireContent::ToolUse {
                        id: "call_1".into(),
                        name: "read".into(),
                        input: json!({"path": "a.rs"}),
                    },
                    WireContent::ToolUse {
                        id: "call_2".into(),
                        name: "read".into(),
                        input: json!({"path": "b.rs"}),
                    },
                ],
            },
            WireMessage {
                role: Role::User,
                content: vec![
                    WireContent::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "a".into(),
                        is_error: false,
                    },
                    WireContent::ToolResult {
                        tool_use_id: "call_2".into(),
                        content: "b".into(),
                        is_error: false,
                    },
                ],
            },
        ];
        let out = wire_to_openai(&messages);

        assert_eq!(out.len(), 3, "one assistant message plus two tool messages");
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[0]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(out[1]["role"], "tool");
        assert_eq!(out[1]["tool_call_id"], "call_1");
        assert_eq!(out[2]["tool_call_id"], "call_2");
    }

    #[test]
    fn tool_arguments_are_a_string_not_an_object() {
        let messages = vec![WireMessage {
            role: Role::Assistant,
            content: vec![WireContent::ToolUse {
                id: "call_1".into(),
                name: "read".into(),
                input: json!({"path": "a.rs"}),
            }],
        }];
        let out = wire_to_openai(&messages);
        let args = out[0]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .expect("arguments must be a JSON string in this dialect");
        assert_eq!(
            serde_json::from_str::<Value>(args).unwrap(),
            json!({"path": "a.rs"})
        );
    }

    #[test]
    fn reasoning_blocks_are_dropped_rather_than_mistranslated() {
        let messages = vec![WireMessage {
            role: Role::Assistant,
            content: vec![
                WireContent::Thinking {
                    thinking: "considering".into(),
                    signature: "sig".into(),
                },
                WireContent::Text {
                    text: "the answer".into(),
                },
            ],
        }];
        let out = wire_to_openai(&messages);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["content"], "the answer");
        assert!(out[0].get("thinking").is_none());
    }
}
