//! The block state machine over the decoded event stream.
//!
//! A tool call's arguments arrive as fragments that are only valid JSON once
//! concatenated, so they are parsed exactly once, at the block's end.

use super::errors::is_context_overflow;
use super::*;

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
                    // A server-side fallback response opens one of these at
                    // each model boundary, as a start/stop pair with no deltas
                    // between them. Treated as text it becomes an empty
                    // assistant message in the transcript — a block that never
                    // said anything, echoed back on every subsequent request.
                    "fallback" => return Ok(()),
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

pub(super) fn index_of(v: &Value) -> u32 {
    v.get("index").and_then(Value::as_u64).unwrap_or(0) as u32
}

pub(super) fn string_field(v: Option<&Value>, key: &str) -> String {
    v.and_then(|d| d.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub(super) fn parse_usage(v: &Value) -> Usage {
    let get = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_creation_input_tokens: get("cache_creation_input_tokens"),
        cache_read_input_tokens: get("cache_read_input_tokens"),
    }
}

/// A refusal is a normal outcome on the success path, never a `ProviderError`.
pub(super) fn parse_stop_reason(reason: &str, category: Option<String>) -> StopReason {
    match reason {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::Refusal { category },
        "pause_turn" => StopReason::PauseTurn,
        _ => StopReason::EndTurn,
    }
}

pub(super) fn mid_stream_error(etype: &str, message: &str) -> ProviderError {
    match etype {
        "overloaded_error" => ProviderError::Overloaded,
        "rate_limit_error" => ProviderError::RateLimited { retry_after: None },
        "authentication_error" | "permission_error" => ProviderError::Auth(Redacted::new(message)),
        _ if is_context_overflow(message) => ProviderError::ContextOverflow,
        _ => ProviderError::Transport(Redacted::new(format!("{etype}: {message}"))),
    }
}
