//! Making tool output safe and affordable to put in a context window.
//!
//! Three jobs: strip control bytes that would corrupt a terminal or confuse the
//! model, cap what goes into the request, and keep the part that was cut so it
//! is not simply lost.
//!
//! This lives in the loop rather than in each tool deliberately. A tool that
//! forgets to truncate puts ten megabytes in the next request and nothing
//! errors; applying it at one choke point means a tool cannot forget, and a new
//! tool inherits the behaviour without knowing it exists.

use std::path::{Path, PathBuf};

use crate::tool::{ToolError, ToolOutput};

/// Remove ANSI escape sequences and stray control characters.
///
/// Tool output reaches two places that both care: a terminal, where a stray
/// escape can move the cursor or change the caller's colours for good, and the
/// model's context, where escapes are noise it has to read past. Tabs and
/// newlines survive; nothing else below 0x20 does.
pub fn sanitise(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => {
                // CSI (`ESC [`) runs until a byte in 0x40..=0x7e. OSC (`ESC ]`)
                // runs to BEL or ST. Anything else: drop the escape alone.
                match chars.peek() {
                    Some('[') => {
                        chars.next();
                        for c in chars.by_ref() {
                            if ('\u{40}'..='\u{7e}').contains(&c) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        chars.next();
                        while let Some(c) = chars.next() {
                            if c == '\u{7}' {
                                break;
                            }
                            if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            '\n' | '\t' => out.push(ch),
            '\r' => {
                // A lone CR rewrites the current line — progress bars are built
                // from it. Keep the content, drop the cursor movement.
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                out.push('\n');
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {}
            c => out.push(c),
        }
    }
    out
}

/// The result of capping an output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capped {
    pub text: String,
    pub truncated: bool,
    /// Bytes removed from the middle.
    pub dropped: usize,
}

/// Keep the head and the tail, drop the middle.
///
/// Both ends matter: the head has the command and its first errors, the tail
/// has the summary and the exit status. Cutting only the tail loses the answer;
/// cutting only the head loses the question.
pub fn cap(text: &str, limit: usize) -> Capped {
    if text.len() <= limit {
        return Capped {
            text: text.to_owned(),
            truncated: false,
            dropped: 0,
        };
    }

    let half = limit / 2;
    let head_end = floor_char_boundary(text, half);
    let tail_start = ceil_char_boundary(text, text.len() - (limit - half));
    let dropped = tail_start.saturating_sub(head_end);

    Capped {
        text: format!("{}{}", &text[..head_end], &text[tail_start..]),
        truncated: true,
        dropped,
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Cap an output and, when anything was cut, write the whole thing to a file the
/// model can read back.
///
/// Without the spill, a 50MB test run is simply gone. With it, the marker names
/// a path and the `read` tool — which already exists and already takes an
/// offset — is how the model gets the rest.
pub fn finish(
    raw: &str,
    limit: usize,
    spill_dir: Option<&Path>,
    call_id: &str,
) -> Result<ToolOutput, ToolError> {
    let clean = sanitise(raw);
    let capped = cap(&clean, limit);

    if !capped.truncated {
        return Ok(ToolOutput {
            content: capped.text,
            truncated: false,
            spill: None,
        });
    }

    let spill = match spill_dir {
        Some(dir) => {
            let path = spill_path(dir, call_id);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ToolError::Failed(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(&path, &clean)
                .map_err(|e| ToolError::Failed(format!("cannot write {}: {e}", path.display())))?;
            Some(path)
        }
        None => None,
    };

    // Re-split so the marker sits where the cut was, rather than at the end.
    let half = capped.text.len() / 2;
    let split = floor_char_boundary(&capped.text, half);

    // In lines, because `read`'s offset is a line number. A byte count cannot
    // be turned into one, so the model's only safe move was `offset: 1` — which
    // re-injects the head it already had, doubling the transcript at exactly the
    // moment the output was declared too big.
    let head_lines = capped.text[..split].lines().count();
    let tail_lines = capped.text[split..].lines().count();
    let total_lines = clean.lines().count();
    let resume_at = head_lines + 1;
    let omitted_lines = total_lines.saturating_sub(head_lines + tail_lines);

    let marker = match &spill {
        Some(path) => format!(
            "\n\n[{} lines ({} bytes) omitted from the middle. The complete output is \
             {} lines and is at {} — read it with offset {} to continue from here.]\n\n",
            omitted_lines,
            capped.dropped,
            total_lines,
            path.display(),
            resume_at
        ),
        None => format!(
            "\n\n[{omitted_lines} lines ({} bytes) omitted from the middle]\n\n",
            capped.dropped
        ),
    };

    let content = format!(
        "{}{}{}",
        &capped.text[..split],
        marker,
        &capped.text[split..]
    );

    Ok(ToolOutput {
        content,
        truncated: true,
        spill,
    })
}

fn spill_path(dir: &Path, call_id: &str) -> PathBuf {
    // The call id comes from the provider and is not a path component we should
    // trust blindly.
    let safe: String = call_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect();
    let name = if safe.is_empty() {
        "output".to_owned()
    } else {
        safe
    };
    dir.join(format!("{name}.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_colour_sequences() {
        assert_eq!(sanitise("\x1b[31merror\x1b[0m: boom"), "error: boom");
        assert_eq!(sanitise("\x1b[1;32m ok \x1b[0m"), " ok ");
    }

    #[test]
    fn strips_an_osc_title_sequence() {
        // A tool setting the terminal title would otherwise persist after axio
        // exits.
        assert_eq!(sanitise("\x1b]0;pwned\x07done"), "done");
        assert_eq!(sanitise("\x1b]0;pwned\x1b\\done"), "done");
    }

    #[test]
    fn keeps_tabs_and_newlines_and_drops_other_control_bytes() {
        assert_eq!(sanitise("a\tb\nc"), "a\tb\nc");
        assert_eq!(sanitise("a\u{0}b\u{7}c\u{7f}"), "abc");
    }

    #[test]
    fn normalises_carriage_returns() {
        assert_eq!(sanitise("a\r\nb"), "a\nb");
        assert_eq!(sanitise("50%\r100%"), "50%\n100%");
    }

    #[test]
    fn keeps_multibyte_text_intact() {
        assert_eq!(sanitise("日本語 — naïve"), "日本語 — naïve");
    }

    #[test]
    fn short_output_is_untouched() {
        let c = cap("hello", 100);
        assert!(!c.truncated);
        assert_eq!(c.text, "hello");
    }

    #[test]
    fn capping_keeps_both_ends() {
        let text = format!("{}{}{}", "S".repeat(50), "M".repeat(1000), "E".repeat(50));
        let c = cap(&text, 100);
        assert!(c.truncated);
        assert!(c.text.starts_with('S'), "the head must survive");
        assert!(c.text.ends_with('E'), "the tail must survive");
        assert!(c.text.len() <= 100);
        assert!(c.dropped > 900);
    }

    #[test]
    fn capping_never_splits_a_character() {
        // A boundary landing mid-sequence would panic on a naive slice.
        let text = "é".repeat(1000);
        for limit in 1..40 {
            let c = cap(&text, limit);
            assert!(
                c.text.chars().all(|ch| ch == 'é'),
                "limit {limit} corrupted the text"
            );
        }
    }

    #[test]
    fn a_large_output_spills_and_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let raw = "x".repeat(10 * 1024 * 1024);
        let out = finish(&raw, 1024, Some(dir.path()), "toolu_01ABC").unwrap();

        assert!(out.truncated);
        assert!(out.content.len() < 2048, "the request must not carry 10MB");
        let spill = out.spill.expect("a spill file");
        assert_eq!(std::fs::metadata(&spill).unwrap().len(), raw.len() as u64);
        assert!(
            out.content.contains(&spill.display().to_string()),
            "the marker must tell the model where the rest is"
        );
    }

    #[test]
    fn a_hostile_call_id_cannot_escape_the_spill_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = spill_path(dir.path(), "../../etc/passwd");
        assert_eq!(path.parent().unwrap(), dir.path());
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn output_that_fits_has_no_spill_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = finish("small", 1024, Some(dir.path()), "toolu_1").unwrap();
        assert!(!out.truncated);
        assert!(out.spill.is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
