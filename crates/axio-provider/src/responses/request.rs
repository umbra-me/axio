//! Building a request for the Responses dialect.
//!
//! Three differences from chat-completions, each of which is a silent failure
//! rather than an error when got wrong:
//!
//! * The system prompt is `instructions`, a field, not a message with a role.
//! * A tool is **flat** — `{type, name, description, parameters}` — where
//!   chat-completions nests everything under `function`. A nested tool here is
//!   accepted and never called.
//! * `store` must be `false`. This endpoint rejects `true` outright.

use axio_core::provider::{Effort, ModelRequest, Role, WireContent};
use serde_json::{Map, Value, json};

pub fn build_body(req: &ModelRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(req.model));

    let instructions = req
        .system
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !instructions.is_empty() {
        body.insert("instructions".into(), json!(instructions));
    }

    body.insert("input".into(), Value::Array(input_items(&req.messages)));
    body.insert("stream".into(), json!(true));
    // Not a preference. The endpoint answers "Store must be set to false".
    body.insert("store".into(), json!(false));
    // The loop applies one tool at a time and feeds the result back before
    // asking again; several calls in one turn would arrive with nowhere to put
    // the ones it did not run yet.
    body.insert("parallel_tool_calls".into(), json!(false));
    body.insert("max_output_tokens".into(), json!(req.max_tokens));

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
        body.insert("tool_choice".into(), json!("auto"));
    }

    body.insert(
        "reasoning".into(),
        json!({ "effort": effort(req.effort), "summary": "auto" }),
    );

    Value::Object(body)
}

/// This dialect's spelling of depth.
///
/// Four levels, and `xhigh` is not one of them — the same collapse the
/// chat-completions builder makes, and upward for the same reason: mapping the
/// default down would make every request shallower than its configuration says.
fn effort(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::XHigh | Effort::Max => "high",
    }
}

/// The transcript as this dialect's input items.
///
/// A tool result is its own top-level item here rather than content inside a
/// user message, so one message carrying several results becomes several items
/// — the same seam the chat-completions builder has, in a different shape.
fn input_items(messages: &[axio_core::provider::WireMessage]) -> Vec<Value> {
    let mut items = Vec::new();

    for message in messages {
        let mut parts: Vec<Value> = Vec::new();
        let text_type = match message.role {
            Role::User => "input_text",
            Role::Assistant => "output_text",
        };

        for content in &message.content {
            match content {
                WireContent::Text { text } => {
                    parts.push(json!({ "type": text_type, "text": text }));
                }
                WireContent::ToolUse { id, name, input } => {
                    items.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        // A JSON string, not an object. The same trap the other
                        // dialect has, and just as silent.
                        "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                    }));
                }
                WireContent::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    items.push(json!({
                        "type": "function_call_output",
                        "call_id": tool_use_id,
                        "output": content,
                    }));
                }
                // Dropped rather than mistranslated, as elsewhere: the
                // transcript keeps them and the projection already discards
                // blocks minted by another model.
                WireContent::Thinking { .. } => {}
            }
        }

        if !parts.is_empty() {
            items.push(json!({
                "type": "message",
                "role": match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": parts,
            }));
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_core::provider::{SystemBlock, ToolSpec, WireMessage};
    use std::sync::Arc;

    fn request() -> ModelRequest {
        let mut req = ModelRequest::new("gpt-5-codex");
        req.system = Arc::from(vec![SystemBlock {
            text: "be brief".into(),
        }]);
        req
    }

    #[test]
    fn the_system_prompt_is_a_field_not_a_message() {
        let body = build_body(&request());
        assert_eq!(body["instructions"], "be brief");
        assert!(
            body["input"].as_array().unwrap().is_empty(),
            "it must not also appear as an input item"
        );
    }

    /// The endpoint answers "Store must be set to false", so this is not a
    /// preference and must not become one.
    #[test]
    fn store_is_always_false() {
        assert_eq!(build_body(&request())["store"], false);
    }

    /// A tool nested under `function`, as the other dialect wants, is accepted
    /// here and simply never called.
    #[test]
    fn a_tool_is_flat_rather_than_nested() {
        let mut req = request();
        req.tools = Arc::from(vec![ToolSpec {
            name: "read".into(),
            description: "read a file".into(),
            input_schema: json!({"type": "object"}),
        }]);
        let body = build_body(&req);
        let tool = &body["tools"][0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["name"], "read", "the name must be at the top level");
        assert!(tool.get("function").is_none(), "{tool}");
    }

    #[test]
    fn a_tool_result_becomes_its_own_item() {
        let mut req = request();
        req.messages = vec![WireMessage {
            role: Role::User,
            content: vec![
                WireContent::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "done".into(),
                    is_error: false,
                },
                WireContent::Text {
                    text: "and now this".into(),
                },
            ],
        }];
        let items = build_body(&req);
        let items = items["input"].as_array().unwrap();
        assert_eq!(items[0]["type"], "function_call_output");
        assert_eq!(items[0]["call_id"], "call_1");
        assert_eq!(items[1]["type"], "message");
        assert_eq!(items[1]["content"][0]["type"], "input_text");
    }

    #[test]
    fn a_tool_call_carries_its_arguments_as_a_string() {
        let mut req = request();
        req.messages = vec![WireMessage {
            role: Role::Assistant,
            content: vec![WireContent::ToolUse {
                id: "call_9".into(),
                name: "read".into(),
                input: json!({"path": "a.rs"}),
            }],
        }];
        let body = build_body(&req);
        let arguments = body["input"][0]["arguments"]
            .as_str()
            .expect("arguments must be a JSON string in this dialect");
        assert_eq!(
            serde_json::from_str::<Value>(arguments).unwrap(),
            json!({"path": "a.rs"})
        );
    }

    #[test]
    fn every_effort_maps_onto_a_value_this_dialect_takes() {
        const ACCEPTED: [&str; 4] = ["low", "medium", "high", "minimal"];
        for level in [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
        ] {
            assert!(ACCEPTED.contains(&effort(level)), "{level:?}");
        }
    }
}
