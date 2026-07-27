//! What the surface writes above the viewport.
//!
//! Scrollback is the transcript: once a line is here it belongs to the
//! terminal, not to this program — it survives the process, scrolls with the
//! scrollbar and copies with the mouse. Nothing in this file redraws.

use super::*;

impl Tui {
    pub(super) fn width<B: Backend>(&self, terminal: &Terminal<B>) -> u16 {
        terminal.size().map(|s| s.width).unwrap_or(80)
    }

    /// Print into the terminal's own scrollback, above the viewport.
    pub(super) fn push<B: Backend>(
        &self,
        terminal: &mut Terminal<B>,
        lines: Vec<Line<'static>>,
    ) -> Result<(), B::Error> {
        if lines.is_empty() {
            return Ok(());
        }
        let height = lines.len() as u16;
        terminal.insert_before(height, |buf| {
            Paragraph::new(lines).render(buf.area, buf);
        })
    }

    pub(super) fn push_user<B: Backend>(
        &self,
        terminal: &mut Terminal<B>,
        text: &str,
    ) -> Result<(), B::Error> {
        let width = self.width(terminal).saturating_sub(2) as usize;
        let mut lines: Vec<Line<'static>> = wrap(text, width)
            .into_iter()
            .enumerate()
            .map(|(i, l)| {
                Line::from(vec![
                    Span::styled(
                        if i == 0 { "› " } else { "  " },
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(l, Style::default().add_modifier(Modifier::BOLD)),
                ])
            })
            .collect();
        lines.push(Line::raw(""));
        self.push(terminal, lines)
    }

    pub(super) fn push_error<B: Backend>(
        &self,
        terminal: &mut Terminal<B>,
        message: &str,
    ) -> Result<(), B::Error> {
        self.push(
            terminal,
            vec![Line::styled(
                format!("  {message}"),
                Style::default().fg(Color::Red),
            )],
        )
    }

    pub(super) fn banner<B: Backend>(
        &self,
        terminal: &mut Terminal<B>,
        resumed: bool,
    ) -> Result<(), B::Error> {
        // One line: the frame below carries the model and the status bar
        // carries the keys, so repeating either here is furniture.
        let what = if resumed { "resumed" } else { "new session" };
        self.push(
            terminal,
            vec![
                Line::from(vec![
                    Span::styled(
                        "  axio",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {what}  ·  ctrl-d to leave"),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]),
                Line::raw(""),
            ],
        )
    }
}

// ------------------------------------------------------------------- helpers

/// One finished tool call, as a row: a mark carrying the outcome, the tool's
/// name in its own column so a run of calls reads as a list rather than as
/// prose, what it acted on, and how long it took against the right margin.
///
/// The subject is `name:detail` — the same canonical string the permission
/// engine decided on, so what is shown is what was authorised.
pub(super) fn tool_line(subject: &str, status: &ToolStatus, width: usize) -> Option<Line<'static>> {
    let (name, detail) = match subject.split_once(':') {
        Some((name, detail)) => (name.to_owned(), detail.to_owned()),
        None => (subject.to_owned(), String::new()),
    };

    let (mark, colour, note) = match status {
        ToolStatus::Ok { ms, .. } => ("⏺", Color::Green, format!("{ms}ms")),
        ToolStatus::Failed { message } => ("⏺", Color::Red, message.clone()),
        ToolStatus::Denied { message } => ("⊘", Color::Yellow, first_sentence(message).to_owned()),
        ToolStatus::Cancelled => ("⊘", Color::DarkGray, "cancelled".to_owned()),
        _ => return None,
    };

    // The name column is wide enough for the longest tool there is, so the
    // detail always starts in the same place.
    let left = format!("{name:<6}  {detail}");
    let room = width.saturating_sub(4);
    let (left, note) = if markdown::text_width(&left) + markdown::text_width(&note) + 2 > room {
        // No room for both: the outcome wins, since the detail is recoverable
        // from the transcript above and the outcome is not.
        let keep = room.saturating_sub(markdown::text_width(&note) + 2);
        (truncate(&left, keep), note)
    } else {
        (left, note)
    };
    let gap = room
        .saturating_sub(markdown::text_width(&left))
        .saturating_sub(markdown::text_width(&note));

    Some(Line::from(vec![
        Span::raw("  "),
        Span::styled(mark.to_owned(), Style::default().fg(colour)),
        Span::raw(" "),
        Span::styled(left, Style::default().add_modifier(Modifier::DIM)),
        Span::raw(" ".repeat(gap)),
        Span::styled(
            note,
            match status {
                ToolStatus::Ok { .. } => Style::default().add_modifier(Modifier::DIM),
                _ => Style::default().fg(colour),
            },
        ),
    ]))
}

/// Cut a string to a number of columns, marking the cut.
fn truncate(text: &str, width: usize) -> String {
    if markdown::text_width(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cell = markdown::cell_width(c);
        if used + cell > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += cell;
    }
    out.push('…');
    out
}

fn first_sentence(text: &str) -> &str {
    match text.find(". ") {
        Some(end) => &text[..=end],
        None => text,
    }
}

/// Wrap on word boundaries, falling back to a hard break for a word that is
/// wider than the terminal.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(20);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let mut word = word;
            // Measured in columns, not characters: a line of ideographs counted
            // by character is written twice as wide as it was measured, and the
            // end of it is clipped away.
            while markdown::text_width(word) > width {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let mut split = String::new();
                let mut used = 0;
                for c in word.chars() {
                    let cell = markdown::cell_width(c);
                    if used + cell > width {
                        break;
                    }
                    split.push(c);
                    used += cell;
                }
                let taken = split.len();
                out.push(split);
                word = &word[taken..];
            }
            if line.is_empty() {
                line.push_str(word);
            } else if markdown::text_width(&line) + 1 + markdown::text_width(word) <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::replace(&mut line, word.to_owned()));
            }
        }
        out.push(line);
    }
    out
}

/// The evidence an approval rests on, as scrollback lines.
pub(super) fn preview_lines(preview: Option<&Preview>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match preview {
        Some(Preview::Diff { unified, .. }) => {
            for line in unified.lines() {
                let colour = match line.chars().next() {
                    Some('+') => Color::Green,
                    Some('-') => Color::Red,
                    Some('@') => Color::Cyan,
                    _ => Color::DarkGray,
                };
                lines.push(Line::styled(
                    format!("  {line}"),
                    Style::default().fg(colour),
                ));
            }
        }
        Some(Preview::Command { raw, cwd, .. }) => {
            // The raw string, never the word split: the split reads as a
            // simpler command than the one that runs.
            for line in wrap(raw, width) {
                lines.push(Line::styled(
                    format!("  $ {line}"),
                    Style::default().fg(Color::White),
                ));
            }
            lines.push(Line::styled(
                format!("  in {}", cwd.display()),
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        Some(Preview::Text { text }) => {
            for line in wrap(text, width) {
                lines.push(Line::styled(
                    format!("  {line}"),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
        }
        None => {}
    }
    lines.push(Line::raw(""));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_line_puts_the_outcome_against_the_right_margin() {
        let line = tool_line(
            "read:notes.md",
            &ToolStatus::Ok {
                output: String::new(),
                truncated: false,
                spill: None,
                ms: 3,
            },
            40,
        )
        .expect("a line");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with("  ⏺ read    notes.md"), "{text:?}");
        assert!(text.ends_with("3ms"), "{text:?}");
        assert!(line.width() <= 40, "{}", line.width());
    }

    #[test]
    fn a_tool_line_keeps_its_outcome_when_the_detail_will_not_fit() {
        // The detail can be read again in the transcript above; whether the
        // command failed cannot.
        let line = tool_line(
            "bash:a-very-long-command-that-fills-the-whole-terminal-and-more",
            &ToolStatus::Failed {
                message: "exit 1".into(),
            },
            36,
        )
        .expect("a line");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains('…'), "{text:?}");
        assert!(text.ends_with("exit 1"), "{text:?}");
        assert!(line.width() <= 36, "{}", line.width());
    }

    #[test]
    fn wrapping_breaks_on_words_and_never_loses_text() {
        let text = "the quick brown fox jumps over the lazy dog";
        let lines = wrap(text, 20);
        assert!(lines.iter().all(|l| l.chars().count() <= 20));
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn a_word_wider_than_the_terminal_is_broken_rather_than_dropped() {
        let long = "x".repeat(60);
        let lines = wrap(&long, 25);
        assert!(lines.iter().all(|l| l.chars().count() <= 25));
        assert_eq!(lines.concat(), long);
    }

    #[test]
    fn blank_lines_survive_wrapping() {
        assert_eq!(wrap("a\n\nb", 40), vec!["a", "", "b"]);
    }

    #[test]
    fn a_command_preview_shows_what_the_shell_gets() {
        // The word split drops redirects and quoting, so a reviewer would see
        // a harmless `cat` where a file is being overwritten.
        let preview = Preview::Command {
            program: "cat".into(),
            argv: vec!["<<EOF".into(), ">".into(), "greet.py".into()],
            raw: "cat <<'EOF' > greet.py".into(),
            cwd: "/tmp".into(),
        };
        let lines = preview_lines(Some(&preview), 60);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("cat <<'EOF' > greet.py"), "{text}");
    }

    #[test]
    fn a_diff_preview_keeps_the_markers_that_carry_the_meaning() {
        let preview = Preview::Diff {
            path: "a.rs".into(),
            unified: "@@ -1,2 +1,2 @@\n-old\n+new".into(),
            added: 1,
            removed: 1,
        };
        let lines = preview_lines(Some(&preview), 60);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("-old"));
        assert!(text.contains("+new"));
        assert!(text.contains("@@"));
    }
}
