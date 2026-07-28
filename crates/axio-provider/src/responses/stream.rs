//! Turning the Responses event stream into axio's.
//!
//! This dialect names its events rather than numbering phases, and it sends a
//! finished tool call in one piece: `response.output_item.done` carries the
//! whole `arguments` string. Text arrives as deltas and is closed by the same
//! event. So a tool call is expanded here into the start/delta/end the loop
//! expects, rather than the loop learning a second shape.

use axio_core::protocol::Usage;
use axio_core::provider::{BlockKind, ProviderError, StopReason, StreamEvent};
use axio_core::redact::Redacted;
use serde_json::Value;

use crate::sse::SseDecoder;

#[derive(Default)]
pub struct ResponsesStream {
    sse: SseDecoder,
    /// The block index handed to the next thing that opens. The dialect's own
    /// `output_index` counts items, and a reasoning item that produces no
    /// visible block would leave a gap the surface reads as a lost block.
    next: u32,
    /// Which index the currently open text or reasoning block was given.
    open: Option<u32>,
    done: bool,
}

impl ResponsesStream {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        let mut out = Vec::new();
        for frame in self.sse.push(chunk) {
            if frame.data.trim().is_empty() || frame.data.trim() == "[DONE]" {
                continue;
            }
            let parsed: Value =
                serde_json::from_str(&frame.data).map_err(|source| ProviderError::Decode {
                    raw: Redacted::new(frame.data.clone()),
                    source,
                })?;
            self.event(&parsed, &mut out)?;
        }
        Ok(out)
    }

    /// A truncated stream is a truncation, never a decode error.
    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        if self.done {
            return Ok(Vec::new());
        }
        Err(ProviderError::Truncated)
    }

    fn event(&mut self, event: &Value, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match kind {
            "response.output_item.added" => {
                let item_type = item_field(event, "type").unwrap_or_default();
                // A function call is not opened here. It arrives complete at
                // `done`, and opening a block now would leave one hanging for
                // every call whose `done` names a different index.
                if item_type == "message" {
                    let index = self.take_index();
                    self.open = Some(index);
                    out.push(StreamEvent::BlockStart {
                        index,
                        kind: BlockKind::Text,
                    });
                } else if item_type == "reasoning" {
                    let index = self.take_index();
                    self.open = Some(index);
                    out.push(StreamEvent::BlockStart {
                        index,
                        kind: BlockKind::Thinking,
                    });
                }
            }

            "response.output_text.delta" => {
                if let Some(text) = event.get("delta").and_then(Value::as_str) {
                    let index = self.open_or_start(out, BlockKind::Text);
                    out.push(StreamEvent::TextDelta {
                        index,
                        text: text.to_owned(),
                    });
                }
            }

            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(text) = event.get("delta").and_then(Value::as_str) {
                    let index = self.open_or_start(out, BlockKind::Thinking);
                    out.push(StreamEvent::ReasoningDelta {
                        index,
                        text: text.to_owned(),
                    });
                }
            }

            "response.output_item.done" => self.item_done(event, out),

            "response.completed" | "response.done" | "response.incomplete" => {
                self.close_open(out);
                if let Some(usage) = event.pointer("/response/usage") {
                    out.push(StreamEvent::Usage(usage_from(usage)));
                }
                self.done = true;
                out.push(StreamEvent::Done {
                    stop: if kind == "response.incomplete" {
                        StopReason::MaxTokens
                    } else {
                        StopReason::EndTurn
                    },
                });
            }

            "response.failed" => {
                let why = event
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("the response failed");
                return Err(ProviderError::Transport(Redacted::new(why.to_owned())));
            }

            // Everything else — created, in_progress, part.added, the audio
            // events — is narration this surface has no use for.
            _ => {}
        }
        Ok(())
    }

    fn item_done(&mut self, event: &Value, out: &mut Vec<StreamEvent>) {
        let item_type = item_field(event, "type").unwrap_or_default();
        if item_type != "function_call" {
            self.close_open(out);
            return;
        }

        // Complete in one event, so the three the loop expects are made here.
        // `call_id` and not `id`: the id names the output item, and answering
        // with it produces a result the model cannot match to its call.
        let call_id = item_field(event, "call_id")
            .or_else(|| item_field(event, "id"))
            .unwrap_or_default();
        let name = item_field(event, "name").unwrap_or_default();
        let arguments = item_field(event, "arguments").unwrap_or_else(|| "{}".to_owned());

        self.close_open(out);
        let index = self.take_index();
        out.push(StreamEvent::BlockStart {
            index,
            kind: BlockKind::ToolUse { id: call_id, name },
        });
        out.push(StreamEvent::ToolInputDelta {
            index,
            json: arguments,
        });
        out.push(StreamEvent::BlockEnd { index });
    }

    /// The index of the open block, opening one first if the stream sent a
    /// delta without an `added` — which the endpoint does for the first chunk
    /// of some responses.
    fn open_or_start(&mut self, out: &mut Vec<StreamEvent>, kind: BlockKind) -> u32 {
        match self.open {
            Some(index) => index,
            None => {
                let index = self.take_index();
                self.open = Some(index);
                out.push(StreamEvent::BlockStart { index, kind });
                index
            }
        }
    }

    fn close_open(&mut self, out: &mut Vec<StreamEvent>) {
        if let Some(index) = self.open.take() {
            out.push(StreamEvent::BlockEnd { index });
        }
    }

    fn take_index(&mut self) -> u32 {
        let index = self.next;
        self.next += 1;
        index
    }
}

fn item_field(event: &Value, name: &str) -> Option<String> {
    event.get("item")?.get(name)?.as_str().map(str::to_owned)
}

/// This dialect's token counts.
///
/// `input_tokens` here already includes what was served from cache, so the
/// cached figure is reported beside it rather than added to it.
fn usage_from(usage: &Value) -> Usage {
    let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        cache_read_input_tokens: usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        // Nothing here reports what a cache write cost; this dialect bills a
        // subscription, so the figure does not exist rather than being zero.
        cache_creation_input_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(stream: &mut ResponsesStream, events: &[Value]) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        for event in events {
            let frame = format!("data: {event}\n\n");
            out.extend(stream.push(frame.as_bytes()).expect("a frame"));
        }
        out
    }

    #[test]
    fn text_arrives_as_one_block() {
        let mut stream = ResponsesStream::default();
        let out = feed(
            &mut stream,
            &[
                serde_json::json!({"type":"response.output_item.added","item":{"type":"message"}}),
                serde_json::json!({"type":"response.output_text.delta","delta":"hel"}),
                serde_json::json!({"type":"response.output_text.delta","delta":"lo"}),
                serde_json::json!({"type":"response.output_item.done","item":{"type":"message"}}),
                serde_json::json!({"type":"response.completed","response":{"usage":{}}}),
            ],
        );

        assert!(matches!(
            out[0],
            StreamEvent::BlockStart {
                index: 0,
                kind: BlockKind::Text
            }
        ));
        let text: String = out
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "hello");
        assert!(matches!(out.last(), Some(StreamEvent::Done { .. })));
    }

    /// The dialect delivers a finished call in one event; the loop expects
    /// start, arguments, end.
    #[test]
    fn a_finished_tool_call_becomes_three_events() {
        let mut stream = ResponsesStream::default();
        let out = feed(
            &mut stream,
            &[serde_json::json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": "call_7",
                    "name": "read",
                    "arguments": "{\"path\":\"a.rs\"}"
                }
            })],
        );

        assert_eq!(out.len(), 3);
        match &out[0] {
            StreamEvent::BlockStart {
                kind: BlockKind::ToolUse { id, name },
                ..
            } => {
                // `call_id`, not `id`: answering with the item id gives the
                // model a result it cannot match to its own call.
                assert_eq!(id, "call_7");
                assert_eq!(name, "read");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(out[1], StreamEvent::ToolInputDelta { .. }));
        assert!(matches!(out[2], StreamEvent::BlockEnd { .. }));
    }

    /// Some responses send the first delta with no `added` before it.
    #[test]
    fn a_delta_without_an_opening_still_opens_a_block() {
        let mut stream = ResponsesStream::default();
        let out = feed(
            &mut stream,
            &[serde_json::json!({"type":"response.output_text.delta","delta":"hi"})],
        );
        assert!(matches!(out[0], StreamEvent::BlockStart { index: 0, .. }));
        assert!(matches!(out[1], StreamEvent::TextDelta { index: 0, .. }));
    }

    #[test]
    fn usage_is_surfaced_so_a_budget_can_be_enforced() {
        let mut stream = ResponsesStream::default();
        let out = feed(
            &mut stream,
            &[serde_json::json!({
                "type": "response.completed",
                "response": {"usage": {
                    "input_tokens": 12,
                    "output_tokens": 34,
                    "input_tokens_details": {"cached_tokens": 5}
                }}
            })],
        );
        let usage = out
            .iter()
            .find_map(|e| match e {
                StreamEvent::Usage(u) => Some(*u),
                _ => None,
            })
            .expect("usage");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 34);
        assert_eq!(usage.cache_read_input_tokens, 5);
    }

    #[test]
    fn a_stream_that_never_completed_is_truncated_not_decoded_wrong() {
        let mut stream = ResponsesStream::default();
        let _ = feed(
            &mut stream,
            &[serde_json::json!({"type":"response.output_text.delta","delta":"hi"})],
        );
        assert!(matches!(stream.finish(), Err(ProviderError::Truncated)));
    }

    #[test]
    fn a_failure_event_is_an_error_rather_than_a_quiet_end() {
        let mut stream = ResponsesStream::default();
        let frame = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"nope\"}}}\n\n";
        let err = stream.push(frame.as_bytes()).expect_err("an error");
        assert!(format!("{err}").contains("nope"), "{err}");
    }
}
