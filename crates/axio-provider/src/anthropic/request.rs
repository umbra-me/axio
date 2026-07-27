//! Building the request body.
//!
//! The bytes are load-bearing: prompt caching is a prefix match, so a body that
//! serialises differently for the same conversation silently costs the whole
//! cached prefix and nothing errors.

use super::*;

/// Build the request body.
///
/// Two properties are load-bearing and both are asserted by tests:
///
/// * `temperature`, `top_p`, `top_k` and `budget_tokens` have no field here at
///   all. They are 400s on this model, and a knob that cannot be expressed
///   cannot be reintroduced by a config layer.
/// * Key order is insertion order (`serde_json/preserve_order`), so the same
///   logical request serialises to the same bytes every run. Caching is a
///   prefix match; non-deterministic JSON silently costs the whole prefix.
pub fn build_body(req: &ModelRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(req.model));
    body.insert("max_tokens".into(), json!(req.max_tokens));
    body.insert("stream".into(), json!(true));

    if !req.system.is_empty() {
        let last = req.system.len() - 1;
        let system: Vec<Value> = req
            .system
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let mut b = Map::new();
                b.insert("type".into(), json!("text"));
                b.insert("text".into(), json!(block.text));
                if req.cache.system && i == last {
                    b.insert("cache_control".into(), ephemeral());
                }
                Value::Object(b)
            })
            .collect();
        body.insert("system".into(), Value::Array(system));
    }

    body.insert("messages".into(), json!(wire_messages(req)));

    if !req.tools.is_empty() {
        let last = req.tools.len() - 1;
        let tools: Vec<Value> = req
            .tools
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let mut t = Map::new();
                t.insert("name".into(), json!(spec.name));
                t.insert("description".into(), json!(spec.description));
                t.insert("input_schema".into(), spec.input_schema.clone());
                if req.cache.tools && i == last {
                    t.insert("cache_control".into(), ephemeral());
                }
                Value::Object(t)
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }

    body.insert(
        "output_config".into(),
        json!({ "effort": req.effort.as_wire() }),
    );

    // Thinking is on by default on this model and is never disabled: disabling
    // it is a 400 above `high` effort, and even where legal it makes the model
    // occasionally write a tool call into visible text — the turn succeeds and
    // the call silently never runs. We only ever ask for a readable summary.
    if req.reasoning == ReasoningDisplay::Summarized {
        body.insert(
            "thinking".into(),
            json!({ "type": "adaptive", "display": "summarized" }),
        );
    }

    Value::Object(body)
}

pub(super) fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

/// Project the wire messages, placing the rolling cache breakpoint if the plan
/// asks for one.
pub(super) fn wire_messages(req: &ModelRequest) -> Vec<Value> {
    req.messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let last_block = m.content.len().saturating_sub(1);
            let content: Vec<Value> = m
                .content
                .iter()
                .enumerate()
                .map(|(j, c)| {
                    let mut v = wire_content(c);
                    if req.cache.message == Some(i)
                        && j == last_block
                        && let Value::Object(map) = &mut v
                    {
                        map.insert("cache_control".into(), ephemeral());
                    }
                    v
                })
                .collect();
            json!({
                "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
                "content": content,
            })
        })
        .collect()
}

pub(super) fn wire_content(c: &WireContent) -> Value {
    match c {
        WireContent::Text { text } => json!({ "type": "text", "text": text }),
        WireContent::Thinking {
            thinking,
            signature,
        } => json!({ "type": "thinking", "thinking": thinking, "signature": signature }),
        WireContent::ToolUse { id, name, input } => {
            json!({ "type": "tool_use", "id": id, "name": name, "input": input })
        }
        WireContent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        }),
    }
}

/// The cache plan for a turn.
///
/// The lookback window is 20 content blocks, so a tool-heavy turn that adds more
/// than that between breakpoints silently stops finding the previous entry —
/// nothing errors, the bill just goes up. Re-placing every 15 keeps a margin.
pub fn rolling_cache_plan(message_count: usize, blocks_since_last: usize) -> CachePlan {
    CachePlan {
        tools: true,
        system: true,
        message: if message_count == 0 {
            None
        } else if blocks_since_last >= 15 {
            Some(message_count - 1)
        } else {
            None
        },
    }
}
