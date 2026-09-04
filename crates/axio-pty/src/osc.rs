//! The in-band status sequence a hosted program may print.
//!
//! `ESC ] 9999 ; <json> BEL` (or ST). A tool with no hooks can still say
//! what it is doing by printing one; the host reads it off the output and
//! the bytes otherwise pass through untouched.

/// Finds `ESC ] 9999 ; … BEL` (or ST) across chunk boundaries and yields
/// the payloads. Anything else passes through untouched; this only reads.
#[derive(Default)]
pub(crate) struct OscScanner {
    /// Bytes since a possible `ESC ]` that has not yet ended, so a sequence
    /// split across two reads is still one sequence.
    pending: Vec<u8>,
}

impl OscScanner {
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        const PREFIX: &[u8] = b"\x1b]9999;";
        let mut out = Vec::new();
        let mut data: Vec<u8> = std::mem::take(&mut self.pending);
        data.extend_from_slice(chunk);
        let mut from = 0;
        while let Some(at) = find(&data[from..], PREFIX).map(|p| p + from) {
            let body_start = at + PREFIX.len();
            let end = data[body_start..]
                .iter()
                .position(|&b| b == 0x07)
                .map(|p| (body_start + p, 1))
                .or_else(|| find(&data[body_start..], b"\x1b\\").map(|p| (body_start + p, 2)));
            match end {
                Some((stop, len)) => {
                    out.push(String::from_utf8_lossy(&data[body_start..stop]).into_owned());
                    from = stop + len;
                }
                None => {
                    // Unfinished: keep from the prefix on, bounded so a
                    // program printing the prefix and never ending it
                    // cannot grow this forever.
                    let keep = &data[at..];
                    if keep.len() < 4096 {
                        self.pending = keep.to_vec();
                    }
                    return out;
                }
            }
        }
        // No unfinished prefix; but the tail might be the start of one.
        let tail = data.len().saturating_sub(PREFIX.len() - 1);
        if let Some(p) = (tail..data.len()).find(|&i| PREFIX.starts_with(&data[i..])) {
            self.pending = data[p..].to_vec();
        }
        out
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::OscScanner;

    #[test]
    fn a_status_sequence_is_found_even_split_across_reads() {
        let mut s = OscScanner::default();
        assert!(s.feed(b"plain \x1b[31mtext\x1b]99").is_empty());
        let got = s.feed(b"99;{\"status\":\"done\"}\x07after\x1b]9999;{\"a\":1}\x1b\\");
        assert_eq!(got, vec!["{\"status\":\"done\"}", "{\"a\":1}"]);
        assert!(s.feed(b"nothing here").is_empty());
    }
}
