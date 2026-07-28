//! Reading the chat-completions stream.
//!
//! Deltas carry an index rather than a block, and the sentinel that ends the
//! stream is a literal rather than an event.

use super::*;

/// Turns chat-completion chunks into protocol events.
#[derive(Debug, Default)]
pub struct OpenAiStream {
    decoder: SseDecoder,
    /// Blocks already announced, so a `BlockStart` is emitted exactly once.
    started: std::collections::BTreeSet<u32>,
    text_index: Option<u32>,
    saw_done: bool,
    next_index: u32,
}

impl crate::body::EventDecoder for OpenAiStream {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        OpenAiStream::push(self, chunk)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        OpenAiStream::finish(self)
    }
}

impl OpenAiStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        let frames = self.decoder.push(chunk);
        let mut out = Vec::new();
        for frame in frames {
            self.map(&frame.data, &mut out)?;
        }
        Ok(out)
    }

    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        let frames = self.decoder.finish();
        let mut out = Vec::new();
        for frame in frames {
            self.map(&frame.data, &mut out)?;
        }
        // Close any block still open, so the loop sees a well-formed message.
        for index in std::mem::take(&mut self.started) {
            out.push(StreamEvent::BlockEnd { index });
        }
        if !self.saw_done {
            return Err(ProviderError::Truncated);
        }
        Ok(out)
    }

    fn map(&mut self, data: &str, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        let data = data.trim();
        if data.is_empty() {
            return Ok(());
        }
        // This dialect ends with a sentinel rather than an event type.
        if data == "[DONE]" {
            self.saw_done = true;
            return Ok(());
        }

        let v: Value = serde_json::from_str(data).map_err(|source| ProviderError::Decode {
            raw: Redacted::new(data.to_owned()),
            source,
        })?;

        if let Some(error) = v.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(ProviderError::Transport(Redacted::new(message.to_owned())));
        }

        if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
            out.push(StreamEvent::Usage(parse_usage(usage)));
        }

        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
            return Ok(());
        };

        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(Value::as_str)
                && !text.is_empty()
            {
                let index = match self.text_index {
                    Some(i) => i,
                    None => {
                        let i = self.take_index();
                        self.text_index = Some(i);
                        self.started.insert(i);
                        out.push(StreamEvent::BlockStart {
                            index: i,
                            kind: BlockKind::Text,
                        });
                        i
                    }
                };
                out.push(StreamEvent::TextDelta {
                    index,
                    text: text.to_owned(),
                });
            }

            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    // The dialect's own per-call index, offset so it cannot
                    // collide with the text block.
                    let raw = call.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let index = raw + 1_000;

                    if self.started.insert(index) {
                        out.push(StreamEvent::BlockStart {
                            index,
                            kind: BlockKind::ToolUse {
                                id: call
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                name: call
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                            },
                        });
                    }
                    if let Some(args) = call
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(Value::as_str)
                        && !args.is_empty()
                    {
                        out.push(StreamEvent::ToolInputDelta {
                            index,
                            json: args.to_owned(),
                        });
                    }
                }
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            for index in std::mem::take(&mut self.started) {
                out.push(StreamEvent::BlockEnd { index });
            }
            self.text_index = None;
            out.push(StreamEvent::Done {
                stop: match reason {
                    "tool_calls" | "function_call" => StopReason::ToolUse,
                    "length" => StopReason::MaxTokens,
                    "content_filter" => StopReason::Refusal { category: None },
                    _ => StopReason::EndTurn,
                },
            });
        }
        Ok(())
    }

    fn take_index(&mut self) -> u32 {
        let i = self.next_index;
        self.next_index += 1;
        i
    }
}

pub(super) fn parse_usage(v: &Value) -> Usage {
    let get = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input_tokens: get("prompt_tokens"),
        output_tokens: get("completion_tokens"),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    }
}
