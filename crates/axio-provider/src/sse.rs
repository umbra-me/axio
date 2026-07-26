//! A hand-written Server-Sent Events decoder.
//!
//! The one property that matters: bytes arrive in arbitrary chunks, and a line
//! terminator can be split across two of them. `\r\n` is the case that bites —
//! a decoder that treats a trailing `\r` as a complete line emits a spurious
//! empty line and dispatches a half-built frame. So a `\r` at the end of the
//! buffer is held until the next byte arrives (or the stream ends), because
//! only then can we tell `\r\n` from a bare `\r`.

/// One dispatched SSE frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    event: Option<String>,
    data: String,
    saw_data: bool,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk. Returns every frame that became complete.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
        self.buf.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some(line) = self.take_line(false) {
            if let Some(frame) = self.consume_line(&line) {
                frames.push(frame);
            }
        }
        frames
    }

    /// The stream ended.
    ///
    /// An unterminated trailing line is **discarded**, per the SSE spec. That is
    /// not pedantry: a connection cut mid-`data:` leaves half a JSON document,
    /// and dispatching it would surface as a decode error — a fatal-looking
    /// failure that hides the real, retryable one. Dropping it lets the caller
    /// observe the missing terminator and report truncation instead.
    ///
    /// A frame whose fields are complete but which never saw its blank line is
    /// still dispatched: those lines were terminated, so nothing is half-read.
    pub fn finish(&mut self) -> Vec<SseFrame> {
        let mut frames = Vec::new();
        // At EOF a trailing `\r` is no longer ambiguous — no `\n` is coming —
        // so resolve it rather than stranding the line before it.
        while let Some(line) = self.take_line(true) {
            if let Some(frame) = self.consume_line(&line) {
                frames.push(frame);
            }
        }

        if !self.buf.is_empty() {
            // What is left has no terminator at all: the connection was cut
            // inside a line. Discard it, and discard the pending frame's
            // already-read fields with it — they belong to a frame we will
            // never see whole, and emitting one with a missing `data:` would
            // surface as a decode error instead of the truncation it is.
            self.buf.clear();
            self.event = None;
            self.data.clear();
            self.saw_data = false;
            return frames;
        }

        frames.extend(self.dispatch());
        frames
    }

    /// Extract one line.
    ///
    /// Returns `None` when the buffer holds no terminator this call can
    /// resolve. Mid-stream, a trailing `\r` is undecidable — the next chunk may
    /// open with `\n` — so it waits. At EOF there is no next chunk, so it
    /// resolves as a bare-CR terminator.
    fn take_line(&mut self, at_eof: bool) -> Option<Vec<u8>> {
        let idx = self.buf.iter().position(|&b| b == b'\n' || b == b'\r')?;

        let (line_end, consume_to) = match self.buf[idx] {
            b'\n' => (idx, idx + 1),
            b'\r' if idx + 1 < self.buf.len() => {
                if self.buf[idx + 1] == b'\n' {
                    (idx, idx + 2)
                } else {
                    (idx, idx + 1)
                }
            }
            b'\r' if at_eof => (idx, idx + 1),
            b'\r' => return None,
            _ => unreachable!("position matched one of the two terminators"),
        };

        let line = self.buf[..line_end].to_vec();
        self.buf.drain(..consume_to);
        Some(line)
    }

    fn consume_line(&mut self, line: &[u8]) -> Option<SseFrame> {
        if line.is_empty() {
            return self.dispatch();
        }
        // Comments (`: keep-alive`) are ignored, per the spec.
        if line[0] == b':' {
            return None;
        }

        let (field, value) = match line.iter().position(|&b| b == b':') {
            Some(colon) => {
                let value = &line[colon + 1..];
                // Exactly one leading space is stripped.
                let value = value.strip_prefix(b" ").unwrap_or(value);
                (&line[..colon], value)
            }
            None => (line, &[][..]),
        };

        // Lossy is correct here: a provider emitting invalid UTF-8 should
        // produce a decode error downstream, not kill the connection.
        let value = String::from_utf8_lossy(value);
        match field {
            b"event" => self.event = Some(value.into_owned()),
            b"data" => {
                if self.saw_data {
                    self.data.push('\n');
                }
                self.data.push_str(&value);
                self.saw_data = true;
            }
            // `id` and `retry` are meaningful in the SSE spec but unused by this
            // API; ignoring them is deliberate, not an oversight.
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseFrame> {
        if !self.saw_data && self.event.is_none() {
            return None;
        }
        let frame = SseFrame {
            event: self.event.take(),
            data: std::mem::take(&mut self.data),
        };
        self.saw_data = false;
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(chunks: &[&[u8]]) -> Vec<SseFrame> {
        let mut d = SseDecoder::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(d.push(c));
        }
        out.extend(d.finish());
        out
    }

    #[test]
    fn decodes_a_simple_frame() {
        let frames = decode_all(&[b"event: ping\ndata: {}\n\n"]);
        assert_eq!(
            frames,
            vec![SseFrame {
                event: Some("ping".into()),
                data: "{}".into()
            }]
        );
    }

    #[test]
    fn joins_multiple_data_lines_with_newline() {
        let frames = decode_all(&[b"data: a\ndata: b\n\n"]);
        assert_eq!(frames[0].data, "a\nb");
    }

    #[test]
    fn ignores_comments() {
        let frames = decode_all(&[b": keep-alive\ndata: x\n\n"]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "x");
    }

    #[test]
    fn strips_exactly_one_leading_space() {
        let frames = decode_all(&[b"data:  two spaces\n\n"]);
        assert_eq!(frames[0].data, " two spaces");
    }

    #[test]
    fn handles_crlf_split_across_chunks() {
        // The `\r` ends chunk one; the `\n` opens chunk two.
        let frames = decode_all(&[b"data: hello\r", b"\n\r\n"]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "hello");
    }

    #[test]
    fn handles_bare_cr_terminator() {
        let frames = decode_all(&[b"data: hello\r\r"]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "hello");
    }

    #[test]
    fn a_field_with_no_colon_is_a_bare_field_name() {
        // "data" alone means an empty data line.
        let frames = decode_all(&[b"data\n\n"]);
        assert_eq!(frames[0].data, "");
    }

    #[test]
    fn finish_flushes_complete_fields_that_never_saw_a_blank_line() {
        // Both lines are terminated; only the dispatching blank line is absent.
        let frames = decode_all(&[b"event: message_stop\ndata: {}\n"]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event.as_deref(), Some("message_stop"));
        assert_eq!(frames[0].data, "{}");
    }

    #[test]
    fn finish_discards_a_line_cut_mid_flight() {
        // Half a JSON document. Dispatching it would produce a decode error
        // that hides the truncation it actually represents.
        let frames = decode_all(&[b"event: message_delta\ndata: {\"type\":\"mess"]);
        assert!(
            frames.is_empty(),
            "an unterminated line must be discarded, not dispatched"
        );
    }

    /// The acceptance property: identical output regardless of chunking.
    #[test]
    fn identical_output_when_split_at_every_byte_boundary() {
        let fixture: &[u8] = include_bytes!("../tests/fixtures/turn.sse");
        let whole = decode_all(&[fixture]);
        assert!(!whole.is_empty(), "fixture decoded to nothing");

        for split in 0..fixture.len() {
            let (a, b) = fixture.split_at(split);
            let got = decode_all(&[a, b]);
            assert_eq!(
                got, whole,
                "chunking at byte {split} changed the decoded frames"
            );
        }
    }

    /// Byte-at-a-time is the worst case for a buffering decoder.
    #[test]
    fn identical_output_one_byte_at_a_time() {
        let fixture: &[u8] = include_bytes!("../tests/fixtures/turn.sse");
        let whole = decode_all(&[fixture]);

        let mut d = SseDecoder::new();
        let mut got = Vec::new();
        for byte in fixture {
            got.extend(d.push(&[*byte]));
        }
        got.extend(d.finish());
        assert_eq!(got, whole);
    }
}
