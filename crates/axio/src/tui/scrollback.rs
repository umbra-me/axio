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
        let mut lines: Vec<Line<'static>> = markdown::wrap(text, width)
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

    /// What a slash command had to say.
    ///
    /// Dim and indented, so it reads as the surface answering rather than the
    /// model — the transcript is otherwise entirely a conversation, and a line
    /// that looks like the model said it is a line someone will quote back.
    pub(super) fn push_command_output<B: Backend>(
        &self,
        terminal: &mut Terminal<B>,
        said: &[String],
    ) -> Result<(), B::Error> {
        if said.is_empty() {
            return Ok(());
        }
        let mut lines: Vec<Line<'static>> = said
            .iter()
            .map(|line| {
                Line::styled(
                    format!("  {line}"),
                    Style::default().add_modifier(Modifier::DIM),
                )
            })
            .collect();
        lines.push(Line::raw(""));
        self.push(terminal, lines)
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
    // detail always starts in the same place — and it is never given up. A
    // failure's message is long, and truncating from the left to make room for
    // it used to eat the name, leaving `⏺ … invalid arguments: …` with nothing
    // to say which of six tools had failed. Found by watching a real session.
    let room = width.saturating_sub(4);
    let name_col = format!("{name:<6}");
    let after_name = room.saturating_sub(markdown::text_width(&name_col) + 2);

    // The outcome next, since it is what the detail cannot be recovered from,
    // then whatever is left goes to the detail.
    let note = markdown::truncate(&note, after_name);
    let detail_room = after_name
        .saturating_sub(markdown::text_width(&note))
        .saturating_sub(2);
    let detail = if detail_room > 0 {
        markdown::truncate(&detail, detail_room)
    } else {
        String::new()
    };

    let left = format!("{name_col}  {detail}");
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

fn first_sentence(text: &str) -> &str {
    match text.find(". ") {
        Some(end) => &text[..=end],
        None => text,
    }
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
            for line in markdown::wrap(raw, width) {
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
            for line in markdown::wrap(text, width) {
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
    #[test]
    fn a_long_failure_message_never_costs_the_tool_name() {
        let line = tool_line(
            "grep:fn state_dir",
            &ToolStatus::Failed {
                message: "invalid arguments: unknown arguments `path`, `query`, `max_results` — `grep` takes pattern, glob, hidden".into(),
            },
            110,
        )
        .expect("a line");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // Which tool failed is the first thing a reader needs, and a message
        // long enough to fill the row used to take it away.
        assert!(
            text.starts_with("  ⏺ grep"),
            "the tool name vanished: {text:?}"
        );
        assert!(text.contains("invalid arguments"), "{text:?}");
        assert!(line.width() <= 110, "{}", line.width());
    }
}
