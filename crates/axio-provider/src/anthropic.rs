//! The Anthropic Messages transport.
//!
//! Three things live here: the request builder (whose byte-stability is what
//! keeps the prompt cache alive), the stream state machine that turns SSE frames
//! into [`StreamEvent`]s, and error classification. Everything the loop needs to
//! know about a failure is answered by [`ProviderError::retryable`].

use std::time::Duration;

use axio_core::protocol::Usage;
use axio_core::provider::{
    BlockKind, CachePlan, ModelRequest, ProviderError, ReasoningDisplay, Role, StopReason,
    StreamEvent, WireContent,
};
use axio_core::redact::Redacted;
use serde_json::{Map, Value, json};

use crate::sse::{SseDecoder, SseFrame};

pub const DEFAULT_MODEL: &str = "claude-opus-5";
pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2023-06-01";

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

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

/// Project the wire messages, placing the rolling cache breakpoint if the plan
/// asks for one.
fn wire_messages(req: &ModelRequest) -> Vec<Value> {
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

fn wire_content(c: &WireContent) -> Value {
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

/// Turns SSE frames into protocol events.
///
/// Holds one piece of state beyond the decoder: whether `message_stop` was
/// seen. A stream that ends without it was truncated, and the loop must retry
/// rather than treat a partial answer as complete.
#[derive(Debug, Default)]
pub struct AnthropicStream {
    decoder: SseDecoder,
    saw_message_stop: bool,
}

impl AnthropicStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn saw_message_stop(&self) -> bool {
        self.saw_message_stop
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        let frames = self.decoder.push(chunk);
        self.map_frames(frames)
    }

    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        let frames = self.decoder.finish();
        let events = self.map_frames(frames)?;
        if !self.saw_message_stop {
            return Err(ProviderError::Truncated);
        }
        Ok(events)
    }

    fn map_frames(&mut self, frames: Vec<SseFrame>) -> Result<Vec<StreamEvent>, ProviderError> {
        let mut out = Vec::new();
        for frame in frames {
            self.map_frame(&frame, &mut out)?;
        }
        Ok(out)
    }

    fn map_frame(
        &mut self,
        frame: &SseFrame,
        out: &mut Vec<StreamEvent>,
    ) -> Result<(), ProviderError> {
        if frame.data.is_empty() {
            return Ok(());
        }
        let v: Value =
            serde_json::from_str(&frame.data).map_err(|source| ProviderError::Decode {
                raw: Redacted::new(frame.data.clone()),
                source,
            })?;

        let kind = v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        match kind.as_str() {
            "message_start" => {
                let msg = v.get("message");
                let id = msg
                    .and_then(|m| m.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                out.push(StreamEvent::MessageStart { id });
                if let Some(usage) = msg.and_then(|m| m.get("usage")) {
                    out.push(StreamEvent::Usage(parse_usage(usage)));
                }
            }
            "content_block_start" => {
                let index = index_of(&v);
                let block = v.get("content_block");
                let block_type = block
                    .and_then(|b| b.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let kind = match block_type {
                    "thinking" | "redacted_thinking" => BlockKind::Thinking,
                    "tool_use" => BlockKind::ToolUse {
                        id: block
                            .and_then(|b| b.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        name: block
                            .and_then(|b| b.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    },
                    _ => BlockKind::Text,
                };
                out.push(StreamEvent::BlockStart { index, kind });
            }
            "content_block_delta" => {
                let index = index_of(&v);
                let delta = v.get("delta");
                let delta_type = delta
                    .and_then(|d| d.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match delta_type {
                    "text_delta" => out.push(StreamEvent::TextDelta {
                        index,
                        text: string_field(delta, "text"),
                    }),
                    "thinking_delta" => out.push(StreamEvent::ReasoningDelta {
                        index,
                        text: string_field(delta, "thinking"),
                    }),
                    "input_json_delta" => out.push(StreamEvent::ToolInputDelta {
                        index,
                        json: string_field(delta, "partial_json"),
                    }),
                    "signature_delta" => out.push(StreamEvent::ReasoningSignature {
                        index,
                        signature: string_field(delta, "signature"),
                    }),
                    // An unknown delta kind is forward compatibility, not an
                    // error: a new block type must not break an old client.
                    _ => {}
                }
            }
            "content_block_stop" => out.push(StreamEvent::BlockEnd {
                index: index_of(&v),
            }),
            "message_delta" => {
                if let Some(usage) = v.get("usage") {
                    out.push(StreamEvent::Usage(parse_usage(usage)));
                }
                let delta = v.get("delta");
                if let Some(reason) = delta
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    let category = delta
                        .and_then(|d| d.get("stop_details"))
                        .or_else(|| v.get("stop_details"))
                        .and_then(|d| d.get("category"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    out.push(StreamEvent::Done {
                        stop: parse_stop_reason(reason, category),
                    });
                }
            }
            "message_stop" => self.saw_message_stop = true,
            "ping" => {}
            "error" => {
                let err = v.get("error");
                let etype = err
                    .and_then(|e| e.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let message = err
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Err(mid_stream_error(etype, message));
            }
            _ => {}
        }
        Ok(())
    }
}

fn index_of(v: &Value) -> u32 {
    v.get("index").and_then(Value::as_u64).unwrap_or(0) as u32
}

fn string_field(v: Option<&Value>, key: &str) -> String {
    v.and_then(|d| d.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn parse_usage(v: &Value) -> Usage {
    let get = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_creation_input_tokens: get("cache_creation_input_tokens"),
        cache_read_input_tokens: get("cache_read_input_tokens"),
    }
}

/// A refusal is a normal outcome on the success path, never a `ProviderError`.
fn parse_stop_reason(reason: &str, category: Option<String>) -> StopReason {
    match reason {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::Refusal { category },
        "pause_turn" => StopReason::PauseTurn,
        _ => StopReason::EndTurn,
    }
}

fn mid_stream_error(etype: &str, message: &str) -> ProviderError {
    match etype {
        "overloaded_error" => ProviderError::Overloaded,
        "rate_limit_error" => ProviderError::RateLimited { retry_after: None },
        "authentication_error" | "permission_error" => ProviderError::Auth(Redacted::new(message)),
        _ if is_context_overflow(message) => ProviderError::ContextOverflow,
        _ => ProviderError::Transport(Redacted::new(format!("{etype}: {message}"))),
    }
}

/// Classify a non-2xx response.
///
/// `retry_after` is the header value. It is honoured over any local backoff
/// table, because the server knows when it will accept work again and we do not.
pub fn classify(status: u16, retry_after: Option<&str>, body: &str) -> ProviderError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let etype = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(body);

    match status {
        401 | 403 => ProviderError::Auth(Redacted::new(message)),
        429 => ProviderError::RateLimited {
            retry_after: parse_retry_after(retry_after),
        },
        529 => ProviderError::Overloaded,
        400 if is_context_overflow(message) || etype == "context_window_exceeded_error" => {
            ProviderError::ContextOverflow
        }
        _ => ProviderError::Http {
            status,
            body: Redacted::new(body),
        },
    }
}

/// The message text is the only signal the API gives for an oversize prompt on
/// a 400, so this match is deliberately broad — a false positive costs one
/// compaction pass, a false negative fails the turn.
fn is_context_overflow(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    (m.contains("context")
        && (m.contains("exceed") || m.contains("too long") || m.contains("window")))
        || m.contains("prompt is too long")
        || m.contains("too many tokens")
}

fn parse_retry_after(header: Option<&str>) -> Option<Duration> {
    let raw = header?.trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    // A float is out of spec but has been observed in the wild.
    raw.parse::<f64>()
        .ok()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .map(Duration::from_secs_f64)
}

/// Collects `input_json_delta` fragments and parses them exactly once.
///
/// Parsing per fragment is the obvious mistake: the fragments are arbitrary
/// string slices of a JSON document, so all but the last are invalid on their
/// own. The parse happens at block end and nowhere else, and `parse_count` is
/// exposed so a test can prove it.
#[derive(Debug, Default)]
pub struct ToolInputAccumulator {
    fragments: std::collections::BTreeMap<u32, String>,
    parse_count: usize,
}

impl ToolInputAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, index: u32, fragment: &str) {
        self.fragments.entry(index).or_default().push_str(fragment);
    }

    /// Take the assembled arguments for a block. An empty accumulation is an
    /// empty object, which is what a no-argument tool call streams.
    pub fn finish(&mut self, index: u32) -> Result<Value, ProviderError> {
        let raw = self.fragments.remove(&index).unwrap_or_default();
        self.parse_count += 1;
        if raw.trim().is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_str(&raw).map_err(|source| ProviderError::Decode {
            raw: Redacted::new(raw),
            source,
        })
    }

    pub fn parse_count(&self) -> usize {
        self.parse_count
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
