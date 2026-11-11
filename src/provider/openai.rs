use super::anthropic::parse_input;
use super::{Delta, Reply, Request, Stop, Usage, post_stream};
use crate::message::{Block, Message};
use crate::sse;
use serde_json::{Value, json};
use std::io::BufRead;

/// OpenAI chat completions, which most hosted and local endpoints also speak.
/// `openai` and `openrouter` have built-in endpoints; any other provider name
/// reads `<NAME>_BASE_URL` and an optional `<NAME>_API_KEY`.
pub fn complete(req: &Request, on: &mut dyn FnMut(Delta)) -> Result<Reply, String> {
    let name = req.model.provider.to_uppercase().replace('-', "_");
    let provider = req.model.provider.as_str();
    let base = crate::auth::get(&format!("{name}_BASE_URL"), provider, "baseUrl")
        .or_else(|| match req.model.provider.as_str() {
            "openai" => Some("https://api.openai.com/v1".into()),
            "openrouter" => Some("https://openrouter.ai/api/v1".into()),
            _ => None,
        })
        .ok_or_else(|| format!("set {name}_BASE_URL, or run `axio auth set {provider} baseUrl`"))?;
    let mut headers = Vec::new();
    if let Some(key) = crate::auth::get(&format!("{name}_API_KEY"), provider, "apiKey") {
        headers.push(("authorization", format!("Bearer {key}")));
    }
    let reader = post_stream(
        &format!("{}/chat/completions", base.trim_end_matches('/')),
        &headers,
        &body(req),
    )?;
    parse_stream(reader, req, on)
}

fn body(req: &Request) -> Value {
    let mut messages = vec![json!({"role": "system", "content": req.system})];
    for m in req.messages {
        push_message(&mut messages, m);
    }
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| {
            json!({"type": "function", "function": {
                "name": t.name, "description": t.description, "parameters": t.schema,
            }})
        })
        .collect();
    let mut body = json!({
        "model": req.model.id,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": messages,
        "tools": tools,
    });
    if let Some(effort) = req.effort {
        body["reasoning_effort"] = json!(effort);
    }
    body
}

fn push_message(out: &mut Vec<Value>, m: &Message) {
    match m {
        Message::User { content } => {
            let mut text = String::new();
            for b in content {
                match b {
                    Block::Text { text: t } => {
                        if !text.is_empty() {
                            text.push_str("\n\n");
                        }
                        text.push_str(t)
                    }
                    Block::ToolResult { id, output, .. } => out.push(json!({
                        "role": "tool", "tool_call_id": id, "content": output,
                    })),
                    _ => {}
                }
            }
            if !text.is_empty() {
                out.push(json!({"role": "user", "content": text}));
            }
        }
        Message::Assistant { content } => {
            let text = m.text();
            let calls: Vec<Value> = content
                .iter()
                .filter_map(|b| match b {
                    Block::ToolCall { id, name, input } => Some(json!({
                        "id": id, "type": "function",
                        "function": {"name": name, "arguments": match input {
                            Value::String(raw) => raw.clone(),
                            other => other.to_string(),
                        }},
                    })),
                    _ => None,
                })
                .collect();
            let mut msg = json!({
                "role": "assistant",
                "content": if text.is_empty() { Value::Null } else { json!(text) },
            });
            if !calls.is_empty() {
                msg["tool_calls"] = json!(calls);
            }
            out.push(msg);
        }
    }
}

#[derive(Default)]
struct Call {
    id: String,
    name: String,
    args: String,
}

fn parse_stream<R: BufRead>(
    reader: R,
    req: &Request,
    on: &mut dyn FnMut(Delta),
) -> Result<Reply, String> {
    let mut text = String::new();
    let mut calls: Vec<Call> = Vec::new();
    let mut stop = None;
    let mut usage = Usage::default();

    sse::read(reader, |data| {
        req.check()?;
        if data == "[DONE]" {
            return Ok(false);
        }
        let ev: Value =
            serde_json::from_str(data).map_err(|e| format!("bad stream event: {e}: {data}"))?;
        if let Some(err) = ev.get("error") {
            return Err(format!("provider error: {err}"));
        }
        if let Some(u) = ev.get("usage").filter(|u| u.is_object()) {
            usage.input = u["prompt_tokens"].as_u64().unwrap_or(0);
            usage.output = u["completion_tokens"].as_u64().unwrap_or(0);
            usage.cache_read = u["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0);
        }
        let Some(choice) = ev["choices"].get(0) else {
            return Ok(true);
        };
        let d = &choice["delta"];
        if let Some(s) = d["content"].as_str() {
            text.push_str(s);
            on(Delta::Text(s));
        }
        for tc in d["tool_calls"].as_array().into_iter().flatten() {
            let i = tc["index"].as_u64().unwrap_or(calls.len() as u64) as usize;
            if calls.len() <= i {
                calls.resize_with(i + 1, Call::default);
            }
            let c = &mut calls[i];
            if let Some(id) = tc["id"].as_str() {
                c.id = id.to_string();
            }
            if let Some(n) = tc["function"]["name"].as_str() {
                c.name.push_str(n);
                on(Delta::ToolCall);
            }
            c.args
                .push_str(tc["function"]["arguments"].as_str().unwrap_or(""));
        }
        if let Some(r) = choice["finish_reason"].as_str() {
            stop = Some(match r {
                "stop" => Stop::EndTurn,
                "tool_calls" | "function_call" => Stop::ToolUse,
                "length" => Stop::MaxTokens,
                "content_filter" => Stop::Refusal,
                other => Stop::Other(other.to_string()),
            });
        }
        Ok(true)
    })?;

    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(Block::Text { text });
    }
    for (i, c) in calls.into_iter().enumerate() {
        content.push(Block::ToolCall {
            id: if c.id.is_empty() {
                format!("call_{i}")
            } else {
                c.id
            },
            name: c.name,
            input: parse_input(&c.args),
        });
    }
    Ok(Reply {
        content,
        stop: stop.ok_or("stream ended before the model finished")?,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_and_split_tool_calls() {
        let stream = r#"data: {"choices":[{"delta":{"content":"Hel"}}]}

data: {"choices":[{"delta":{"content":"lo","tool_calls":[{"index":0,"id":"c1","function":{"name":"read","arguments":"{\"pa"}}]}}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]},"finish_reason":"tool_calls"}]}

data: {"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":3}}

data: [DONE]

"#;
        let reply =
            super::super::with_request(false, |r| parse_stream(stream.as_bytes(), r, &mut |_| {}))
                .unwrap();
        assert_eq!(reply.stop, Stop::ToolUse);
        assert_eq!(reply.usage.output, 3);
        assert_eq!(
            reply.content[0],
            Block::Text {
                text: "Hello".into()
            }
        );
        assert_eq!(
            reply.content[1],
            Block::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                input: json!({"path": "a"})
            }
        );
    }

    #[test]
    fn maps_tool_results_to_tool_messages() {
        let mut out = Vec::new();
        push_message(
            &mut out,
            &Message::User {
                content: vec![Block::ToolResult {
                    id: "c1".into(),
                    output: "ok".into(),
                    is_error: false,
                }],
            },
        );
        assert_eq!(
            out,
            [json!({"role":"tool","tool_call_id":"c1","content":"ok"})]
        );
    }
}
