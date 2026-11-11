use super::{Delta, Reply, Request, Stop, Usage, post_stream};
use crate::message::{Block, Message};
use crate::sse;
use serde_json::{Value, json};
use std::io::BufRead;

const PROVIDER: &str = "anthropic";

pub fn complete(req: &Request, on: &mut dyn FnMut(Delta)) -> Result<Reply, String> {
    let base = crate::auth::get("ANTHROPIC_BASE_URL", PROVIDER, "baseUrl")
        .unwrap_or_else(|| "https://api.anthropic.com".into());
    let mut headers = vec![("anthropic-version", "2023-06-01".to_string())];
    if let Some(key) = crate::auth::get("ANTHROPIC_API_KEY", PROVIDER, "apiKey") {
        headers.push(("x-api-key", key));
    } else if let Some(token) = crate::auth::get("ANTHROPIC_AUTH_TOKEN", PROVIDER, "authToken") {
        headers.push(("authorization", format!("Bearer {token}")));
    } else {
        return Err("set ANTHROPIC_API_KEY, or run `axio auth set anthropic`".into());
    }
    let reader = post_stream(
        &format!("{}/v1/messages", base.trim_end_matches('/')),
        &headers,
        &body(req),
    )?;
    parse_stream(reader, req, on)
}

fn body(req: &Request) -> Value {
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.schema,
                "eager_input_streaming": true,
            })
        })
        .collect();
    let mut body = json!({
        "model": req.model.id,
        "max_tokens": 64000,
        "stream": true,
        // Automatic caching moves the breakpoint to the end of the conversation
        // each turn, so every request reads the previous one's prefix.
        "cache_control": {"type": "ephemeral"},
        "system": req.system,
        "tools": tools,
        "messages": req.messages.iter().map(message).collect::<Vec<_>>(),
    });
    if let Some(effort) = req.effort {
        body["output_config"] = json!({ "effort": effort });
    }
    body
}

fn message(m: &Message) -> Value {
    let (role, content) = match m {
        Message::User { content } => ("user", content),
        Message::Assistant { content } => ("assistant", content),
    };
    let blocks: Vec<Value> = content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } if text.is_empty() => None,
            Block::Text { text } => Some(json!({"type": "text", "text": text})),
            Block::ToolCall { id, name, input } => {
                let input = if input.is_object() {
                    input.clone()
                } else {
                    json!({})
                };
                Some(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
            }
            Block::ToolResult {
                id,
                output,
                is_error,
            } => Some(json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": output,
                "is_error": is_error,
            })),
            Block::Opaque { provider, value } if provider == PROVIDER => Some(value.clone()),
            Block::Opaque { .. } => None,
        })
        .collect();
    json!({"role": role, "content": blocks})
}

enum Building {
    Text(String),
    Tool {
        id: String,
        name: String,
        json: String,
    },
    Opaque(Value),
}

pub(crate) fn parse_stream<R: BufRead>(
    reader: R,
    req: &Request,
    on: &mut dyn FnMut(Delta),
) -> Result<Reply, String> {
    let mut blocks: Vec<Option<Building>> = Vec::new();
    let mut stop = None;
    let mut usage = Usage::default();

    sse::read(reader, |data| {
        req.check()?;
        let ev: Value =
            serde_json::from_str(data).map_err(|e| format!("bad stream event: {e}: {data}"))?;
        let index = ev["index"].as_u64().unwrap_or(0) as usize;
        match ev["type"].as_str().unwrap_or("") {
            "message_start" => {
                let u = &ev["message"]["usage"];
                usage.input = u["input_tokens"].as_u64().unwrap_or(0);
                usage.cache_read = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                usage.cache_write = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            }
            "content_block_start" => {
                let cb = &ev["content_block"];
                let b = match cb["type"].as_str().unwrap_or("") {
                    "text" => Building::Text(cb["text"].as_str().unwrap_or("").to_string()),
                    "tool_use" => {
                        let name = cb["name"].as_str().unwrap_or("").to_string();
                        on(Delta::ToolCall);
                        Building::Tool {
                            id: cb["id"].as_str().unwrap_or("").to_string(),
                            name,
                            json: String::new(),
                        }
                    }
                    _ => {
                        if cb["type"] == "thinking" || cb["type"] == "redacted_thinking" {
                            on(Delta::Thinking);
                        }
                        Building::Opaque(cb.clone())
                    }
                };
                if blocks.len() <= index {
                    blocks.resize_with(index + 1, || None);
                }
                blocks[index] = Some(b);
            }
            "content_block_delta" => {
                let d = &ev["delta"];
                match blocks.get_mut(index).and_then(Option::as_mut) {
                    Some(Building::Text(t)) => {
                        let s = d["text"].as_str().unwrap_or("");
                        t.push_str(s);
                        on(Delta::Text(s));
                    }
                    Some(Building::Tool { json, .. }) => {
                        json.push_str(d["partial_json"].as_str().unwrap_or(""));
                    }
                    Some(Building::Opaque(v)) => match d["type"].as_str() {
                        Some("thinking_delta") => append(v, "thinking", &d["thinking"]),
                        Some("signature_delta") => append(v, "signature", &d["signature"]),
                        _ => {}
                    },
                    None => {}
                }
            }
            "message_delta" => {
                if let Some(r) = ev["delta"]["stop_reason"].as_str() {
                    stop = Some(match r {
                        "end_turn" | "stop_sequence" => Stop::EndTurn,
                        "tool_use" => Stop::ToolUse,
                        "max_tokens" => Stop::MaxTokens,
                        "refusal" => Stop::Refusal,
                        other => Stop::Other(other.to_string()),
                    });
                }
                if let Some(n) = ev["usage"]["output_tokens"].as_u64() {
                    usage.output = n;
                }
            }
            "message_stop" => return Ok(false),
            "error" => {
                return Err(format!(
                    "{}: {}",
                    ev["error"]["type"].as_str().unwrap_or("error"),
                    ev["error"]["message"].as_str().unwrap_or(data)
                ));
            }
            _ => {}
        }
        Ok(true)
    })?;

    let stop = stop.ok_or("stream ended before the model finished")?;
    let content = blocks
        .into_iter()
        .flatten()
        .map(|b| match b {
            Building::Text(text) => Block::Text { text },
            Building::Tool { id, name, json } => Block::ToolCall {
                id,
                name,
                input: parse_input(&json),
            },
            Building::Opaque(value) => Block::Opaque {
                provider: PROVIDER.into(),
                value,
            },
        })
        .collect();
    Ok(Reply {
        content,
        stop,
        usage,
    })
}

fn append(v: &mut Value, key: &str, piece: &Value) {
    let s = piece.as_str().unwrap_or("");
    let cur = v[key].as_str().unwrap_or("").to_string();
    v[key] = Value::String(cur + s);
}

/// Eager input streaming skips server-side validation, so a truncated or
/// malformed input arrives as a string that the tool layer reports back.
pub(crate) fn parse_input(json: &str) -> Value {
    if json.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(json).unwrap_or_else(|_| Value::String(json.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = r#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":4,"cache_creation_input_tokens":3}}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}

data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi"}}

data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t1","name":"bash","input":{}}}

data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}

data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}

data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}

data: {"type":"message_stop"}

"#;

    #[test]
    fn assembles_thinking_text_and_tool_calls() {
        let mut text = String::new();
        let reply = super::super::with_request(false, |r| {
            parse_stream(STREAM.as_bytes(), r, &mut |d| {
                if let Delta::Text(t) = d {
                    text.push_str(t)
                }
            })
        })
        .unwrap();
        assert_eq!(text, "Hi");
        assert_eq!(reply.stop, Stop::ToolUse);
        assert_eq!(
            reply.usage,
            Usage {
                input: 10,
                output: 7,
                cache_read: 4,
                cache_write: 3
            }
        );
        assert_eq!(
            reply.content[0],
            Block::Opaque {
                provider: "anthropic".into(),
                value: json!({"type":"thinking","thinking":"hm","signature":"sig"})
            }
        );
        assert_eq!(
            reply.content[2],
            Block::ToolCall {
                id: "t1".into(),
                name: "bash".into(),
                input: json!({"command":"ls"})
            }
        );
    }

    #[test]
    fn replays_own_thinking_and_drops_foreign_blocks() {
        let m = Message::Assistant {
            content: vec![
                Block::Opaque {
                    provider: "anthropic".into(),
                    value: json!({"type":"thinking"}),
                },
                Block::Opaque {
                    provider: "other".into(),
                    value: json!({}),
                },
                Block::Text { text: "".into() },
            ],
        };
        assert_eq!(message(&m)["content"], json!([{"type":"thinking"}]));
    }

    #[test]
    fn reports_stream_errors_and_truncation() {
        let err = super::super::with_request(false, |r| parse_stream(
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\n\n"
                .as_bytes(),
            r,
            &mut |_| {},
        ))
        .unwrap_err();
        assert_eq!(err, "overloaded_error: busy");
        assert!(
            super::super::with_request(false, |r| parse_stream("".as_bytes(), r, &mut |_| {}))
                .is_err()
        );
        let stopped =
            super::super::with_request(true, |r| parse_stream(STREAM.as_bytes(), r, &mut |_| {}));
        assert_eq!(stopped.unwrap_err(), crate::agent::ABORTED);
    }
}
