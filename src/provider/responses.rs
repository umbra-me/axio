//! OpenAI's Responses API, used for ChatGPT sign-ins, which chat completions
//! refuses. Reasoning comes back encrypted and is sent back unchanged, so a
//! model keeps its reasoning across tool calls without OpenAI storing it.

use super::anthropic::parse_input;
use super::{Delta, Reply, Request, Stop, Usage, post_stream};
use crate::message::{Block, Message};
use crate::sse;
use serde_json::{Value, json};
use std::io::BufRead;

pub fn complete(req: &Request, on: &mut dyn FnMut(Delta)) -> Result<Reply, String> {
    let token = crate::chatgpt::bearer()?;
    let reader = post_stream(
        &format!("{}/responses", crate::chatgpt::API),
        &[("authorization", format!("Bearer {token}"))],
        &body(req),
    )?;
    parse_stream(reader, req, on)
}

fn body(req: &Request) -> Value {
    let mut input = Vec::new();
    for m in req.messages {
        push_input(&mut input, m, &req.model.provider);
    }
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| {
            json!({"type": "function", "name": t.name, "description": t.description,
                   "parameters": t.schema})
        })
        .collect();
    let mut reasoning = json!({"summary": "auto"});
    if let Some(effort) = req.effort {
        // `max` is axio's top level; the Responses API calls it `xhigh`.
        reasoning["effort"] = json!(if effort == "max" { "xhigh" } else { effort });
    }
    json!({
        "model": req.model.id,
        "instructions": req.system,
        "input": input,
        "tools": tools,
        "tool_choice": "auto",
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
        "reasoning": reasoning,
    })
}

fn push_input(out: &mut Vec<Value>, m: &Message, provider: &str) {
    match m {
        Message::User { content } => {
            let mut parts = Vec::new();
            for b in content {
                match b {
                    Block::ToolResult { id, output, .. } => out.push(json!({
                        "type": "function_call_output", "call_id": id, "output": output,
                    })),
                    Block::Text { text } => parts.push(json!({"type": "input_text", "text": text})),
                    Block::Image { media_type, data } => parts.push(json!({
                        "type": "input_image",
                        "image_url": format!("data:{media_type};base64,{data}"),
                    })),
                    Block::ToolCall { .. } | Block::Opaque { .. } => {}
                }
            }
            if !parts.is_empty() {
                out.push(json!({"role": "user", "content": parts}));
            }
        }
        Message::Assistant { content } => {
            for b in content {
                match b {
                    Block::Text { text } if !text.is_empty() => out.push(json!({
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    })),
                    Block::ToolCall { id, name, input } => out.push(json!({
                        "type": "function_call", "call_id": id, "name": name,
                        "arguments": match input {
                            Value::String(raw) => raw.clone(),
                            other => other.to_string(),
                        },
                    })),
                    Block::Opaque { provider: p, value } if p == provider => {
                        out.push(value.clone())
                    }
                    _ => {}
                }
            }
        }
    }
}

fn parse_stream<R: BufRead>(
    reader: R,
    req: &Request,
    on: &mut dyn FnMut(Delta),
) -> Result<Reply, String> {
    let mut content = Vec::new();
    let mut stop = None;
    let mut usage = Usage::default();
    sse::read(reader, |data| {
        req.check()?;
        let ev: Value =
            serde_json::from_str(data).map_err(|e| format!("bad stream event: {e}: {data}"))?;
        let item = &ev["item"];
        match ev["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => on(Delta::Text(ev["delta"].as_str().unwrap_or(""))),
            "response.reasoning_summary_text.delta" => {
                on(Delta::ThinkingText(ev["delta"].as_str().unwrap_or("")))
            }
            "response.function_call_arguments.delta" => {
                on(Delta::ToolInput(ev["delta"].as_str().unwrap_or("")))
            }
            "response.output_item.added" => match item["type"].as_str() {
                Some("reasoning") => on(Delta::Thinking),
                Some("function_call") => on(Delta::ToolCall(item["name"].as_str().unwrap_or(""))),
                _ => {}
            },
            // Finished items, in order, are the reply.
            "response.output_item.done" => match item["type"].as_str() {
                Some("message") => {
                    let text: String = item["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["text"].as_str().or(c["refusal"].as_str()))
                        .collect();
                    if !text.is_empty() {
                        content.push(Block::Text { text });
                    }
                }
                Some("function_call") => content.push(Block::ToolCall {
                    id: item["call_id"].as_str().unwrap_or("").to_string(),
                    name: item["name"].as_str().unwrap_or("").to_string(),
                    input: parse_input(item["arguments"].as_str().unwrap_or("")),
                }),
                Some("reasoning") if item["encrypted_content"].is_string() => {
                    content.push(Block::Opaque {
                        provider: req.model.provider.clone(),
                        value: item.clone(),
                    });
                }
                _ => {}
            },
            kind @ ("response.completed" | "response.incomplete") => {
                let r = &ev["response"];
                let u = &r["usage"];
                // OpenAI counts cached tokens inside input_tokens.
                usage.cache_read = u["input_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .unwrap_or(0);
                usage.input = u["input_tokens"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(usage.cache_read);
                usage.output = u["output_tokens"].as_u64().unwrap_or(0);
                stop = Some(if kind == "response.incomplete" {
                    match r["incomplete_details"]["reason"].as_str() {
                        Some("max_output_tokens") => Stop::MaxTokens,
                        Some("content_filter") => Stop::Refusal,
                        other => Stop::Other(other.unwrap_or("incomplete").to_string()),
                    }
                } else {
                    Stop::EndTurn
                });
                return Ok(false);
            }
            "response.failed" => {
                let e = &ev["response"]["error"];
                return Err(format!(
                    "{}: {}",
                    e["code"].as_str().unwrap_or("failed"),
                    e["message"].as_str().unwrap_or("the response failed")
                ));
            }
            "error" => {
                return Err(format!(
                    "{}: {}",
                    ev["code"].as_str().unwrap_or("error"),
                    ev["message"].as_str().unwrap_or(data)
                ));
            }
            _ => {}
        }
        Ok(true)
    })?;
    let mut stop = stop.ok_or("stream ended before the model finished")?;
    if stop == Stop::EndTurn && content.iter().any(|b| matches!(b, Block::ToolCall { .. })) {
        stop = Stop::ToolUse;
    }
    Ok(Reply {
        content,
        stop,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reasoning_text_and_tool_calls_in_order() {
        let stream = r#"data: {"type":"response.output_item.added","item":{"type":"reasoning"}}

data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"enc"}}

data: {"type":"response.output_text.delta","delta":"Hi"}

data: {"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hi"}]}}

data: {"type":"response.output_item.added","item":{"type":"function_call","name":"Read"}}

data: {"type":"response.function_call_arguments.delta","delta":"{\"path\":"}

data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"Read","arguments":"{\"path\":\"a\"}"}}

data: {"type":"response.completed","response":{"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":60},"output_tokens":7}}}

"#;
        let reply =
            super::super::with_request(false, |r| parse_stream(stream.as_bytes(), r, &mut |_| {}))
                .unwrap();
        assert_eq!(reply.stop, Stop::ToolUse);
        assert_eq!(
            (
                reply.usage.input,
                reply.usage.cache_read,
                reply.usage.output
            ),
            (40, 60, 7)
        );
        assert!(
            matches!(&reply.content[0], Block::Opaque { value, .. } if value["encrypted_content"] == "enc")
        );
        assert_eq!(reply.content[1], Block::Text { text: "Hi".into() });
        assert_eq!(
            reply.content[2],
            Block::ToolCall {
                id: "call_1".into(),
                name: "Read".into(),
                input: json!({"path": "a"})
            }
        );
    }

    #[test]
    fn failures_and_cut_off_replies_say_so() {
        let failed = r#"data: {"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"slow down"}}}

"#;
        let err =
            super::super::with_request(false, |r| parse_stream(failed.as_bytes(), r, &mut |_| {}))
                .unwrap_err();
        assert_eq!(err, "rate_limit_exceeded: slow down");
        let cut = r#"data: {"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{}}}

"#;
        let reply =
            super::super::with_request(false, |r| parse_stream(cut.as_bytes(), r, &mut |_| {}))
                .unwrap();
        assert_eq!(reply.stop, Stop::MaxTokens);
    }

    #[test]
    fn history_becomes_input_items_with_reasoning_kept_for_its_provider() {
        let mut out = Vec::new();
        let reasoning =
            json!({"type": "reasoning", "id": "rs_1", "encrypted_content": "enc", "summary": []});
        push_input(
            &mut out,
            &Message::Assistant {
                content: vec![
                    Block::Opaque {
                        provider: "chatgpt".into(),
                        value: reasoning.clone(),
                    },
                    Block::Opaque {
                        provider: "anthropic".into(),
                        value: json!({"type": "thinking"}),
                    },
                    Block::Text {
                        text: "Reading".into(),
                    },
                    Block::ToolCall {
                        id: "call_1".into(),
                        name: "Read".into(),
                        input: json!({"path": "a"}),
                    },
                ],
            },
            "chatgpt",
        );
        push_input(
            &mut out,
            &Message::User {
                content: vec![
                    Block::ToolResult {
                        id: "call_1".into(),
                        output: "text".into(),
                        is_error: false,
                    },
                    Block::Text {
                        text: "next".into(),
                    },
                ],
            },
            "chatgpt",
        );
        assert_eq!(
            out,
            vec![
                reasoning,
                json!({"role": "assistant", "content": [{"type": "output_text", "text": "Reading"}]}),
                json!({"type": "function_call", "call_id": "call_1", "name": "Read", "arguments": "{\"path\":\"a\"}"}),
                json!({"type": "function_call_output", "call_id": "call_1", "output": "text"}),
                json!({"role": "user", "content": [{"type": "input_text", "text": "next"}]}),
            ]
        );
    }
}
