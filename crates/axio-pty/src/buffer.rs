//! What a hosted agent has written, and how much of it a reader has seen.
//!
//! Bounded, and read by cursor rather than pushed. Both halves matter.
//!
//! **Bounded**, because a terminal left running overnight produces more output
//! than a window needs and all of it would otherwise be resident.
//!
//! **By cursor**, because a surface that is only ever *pushed* to loses whatever
//! arrived while nothing was listening — and something is always not listening,
//! every time a webview reloads. A reader that asks "everything after N" gets a
//! correct answer whether it has been away for a frame or a minute. The prior
//! art here pushes only, keeps its scrollback in the frontend, and states in its
//! own architecture notes that missed output is not replayed.

use std::collections::VecDeque;

/// Two megabytes, which is far more than a person scrolls back through and far
/// less than a chatty build leaves behind.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

/// A ring of bytes with a monotonic position.
///
/// Bytes rather than text, deliberately. A `read` from a pipe lands wherever
/// the kernel decided, and decoding each chunk on its own turns any multi-byte
/// character unlucky enough to straddle that boundary into a replacement
/// character — permanently, because the damage is done before anything can
/// reassemble it. Decoding belongs to whoever holds the whole stream.
#[derive(Debug)]
pub struct Ring {
    bytes: VecDeque<u8>,
    /// Total bytes ever written. A cursor is compared against this, so it keeps
    /// meaning after old bytes have been dropped.
    written: u64,
}

impl Default for Ring {
    fn default() -> Self {
        Self::new()
    }
}

impl Ring {
    pub fn new() -> Self {
        Self {
            bytes: VecDeque::new(),
            written: 0,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        self.written += chunk.len() as u64;
        let excess = self.bytes.len().saturating_sub(MAX_BYTES);
        if excess > 0 {
            self.bytes.drain(..excess);
        }
    }

    /// The position a reader that has seen everything would hold.
    pub fn cursor(&self) -> u64 {
        self.written
    }

    /// The oldest position still readable.
    ///
    /// Above zero once the ring has wrapped, and a reader that asks for less
    /// than this is told where the record actually starts rather than handed a
    /// silently incomplete answer.
    pub fn earliest(&self) -> u64 {
        self.written - self.bytes.len() as u64
    }

    /// Everything after `from`, and the cursor to ask with next time.
    ///
    /// A reader that has fallen behind the ring is given what survives and the
    /// position it really starts at — so the gap is visible rather than a
    /// stretch of output that appears never to have happened.
    pub fn read_from(&self, from: u64) -> (Vec<u8>, u64) {
        let earliest = self.earliest();
        let start = from.max(earliest);
        let offset = (start - earliest) as usize;
        let out: Vec<u8> = self.bytes.iter().skip(offset).copied().collect();
        (out, self.written)
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// `bytes` with every terminal *query* removed: the sequences a program
/// sends to ask the terminal about itself — device attributes, cursor
/// position, mode state, colours, the keyboard protocol, a DECRQSS. Replayed
/// to a fresh emulator on reattach, each one would be answered again, and
/// the answers would land on the program's stdin as keystrokes it did not
/// ask for now. The live stream is not filtered: a live question deserves
/// its answer.
pub fn strip_queries(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b
            && let Some(len) = query_len(&bytes[i..])
        {
            i += len;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// The length of the query sequence at the start of `s` (which begins with
/// ESC), or `None` when it is not a query.
fn query_len(s: &[u8]) -> Option<usize> {
    let second = *s.get(1)?;
    match second {
        // CSI: parameters, then a final byte. Queries: `c` (DA1/DA2),
        // `6n` (DSR), `?…$p` (DECRQM), `?u` (kitty keyboard query).
        b'[' => {
            let mut j = 2;
            while j < s.len()
                && (s[j].is_ascii_digit() || matches!(s[j], b';' | b'?' | b'>' | b'=' | b'$'))
            {
                j += 1;
            }
            let end = *s.get(j)?;
            let params = &s[2..j];
            let is_query = match end {
                b'c' => true,
                b'n' => params == b"6" || params == b"?6",
                b'p' => params.starts_with(b"?") && params.ends_with(b"$"),
                b'u' => params == b"?",
                _ => false,
            };
            is_query.then_some(j + 1)
        }
        // OSC colour queries: `10;?`, `11;?`, `12;?`, `4;n;?`, ended by BEL or ST.
        b']' => {
            let body_end = s
                .iter()
                .position(|&b| b == 0x07)
                .or_else(|| s.windows(2).position(|w| w == b"\x1b\\").map(|p| p + 1))?;
            let body = &s[2..body_end];
            let is_query = body.ends_with(b";?") && body.first().is_some_and(u8::is_ascii_digit);
            is_query.then_some(body_end + 1)
        }
        // DCS `$q` (DECRQSS), ended by ST.
        b'P' => {
            if !s.get(2..4).is_some_and(|x| x == b"$q") {
                return None;
            }
            let st = s.windows(2).position(|w| w == b"\x1b\\")?;
            Some(st + 2)
        }
        _ => None,
    }
}

#[cfg(test)]
mod query_tests {
    use super::strip_queries;

    #[test]
    fn queries_go_and_everything_else_stays() {
        let live = b"hello\x1b[31mred\x1b[0m \x1b[2J\x1b[H";
        assert_eq!(strip_queries(live), live.to_vec());
        let asked = b"a\x1b[cb\x1b[>0cc\x1b[6nd\x1b[?2026$pe\x1b[?uf\x1b]11;?\x07g\x1bP$qm\x1b\\h";
        assert_eq!(strip_queries(asked), b"abcdefgh".to_vec());
        // An unterminated sequence at the very end is kept as it is.
        assert_eq!(strip_queries(b"x\x1b["), b"x\x1b[".to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_gets_only_what_it_has_not_seen() {
        let mut ring = Ring::new();
        ring.push(b"hello ");
        let (first, cursor) = ring.read_from(0);
        assert_eq!(first, b"hello ");

        ring.push(b"world");
        let (second, next) = ring.read_from(cursor);
        assert_eq!(second, b"world", "not the whole stream again");
        assert_eq!(next, 11);

        // Nothing new: an empty answer, not a repeat.
        assert_eq!(ring.read_from(next).0, b"");
    }

    /// The case a webview reload is: away for a while, then ask for the gap.
    #[test]
    fn output_written_while_nobody_was_reading_survives() {
        let mut ring = Ring::new();
        let cursor = ring.read_from(0).1;
        for _ in 0..100 {
            ring.push(b"tick ");
        }
        let (missed, _) = ring.read_from(cursor);
        assert_eq!(missed.len(), 500, "a reader that was away misses nothing");
    }

    #[test]
    fn the_ring_is_bounded_and_says_where_it_now_starts() {
        let mut ring = Ring::new();
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..40 {
            ring.push(&chunk);
        }
        assert!(ring.bytes.len() <= MAX_BYTES);
        assert!(ring.earliest() > 0, "it wrapped");

        // A reader stranded before the window gets what survives, and a cursor
        // that tells it where the record really begins.
        let (out, cursor) = ring.read_from(0);
        assert_eq!(out.len(), ring.bytes.len());
        assert_eq!(cursor, ring.cursor());
    }

    /// Bytes, not text. A chunk boundary must not be able to destroy a
    /// character, which is exactly what per-chunk lossy decoding does.
    #[test]
    fn a_multibyte_character_split_across_chunks_survives() {
        let mut ring = Ring::new();
        let text = "héllo → 世界";
        let bytes = text.as_bytes();
        for chunk in bytes.chunks(3) {
            ring.push(chunk);
        }
        let (out, _) = ring.read_from(0);
        assert_eq!(
            String::from_utf8(out).expect("bytes are intact"),
            text,
            "decoding per chunk would have replaced the split characters"
        );
    }
}
