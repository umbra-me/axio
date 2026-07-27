//! The four rows the surface owns and repaints.
//!
//! Everything here is a pure function of the state and the area it was given.
//! What is settled has already gone to scrollback; this draws only what is
//! still in flight — the streaming tail, the composer, and what is happening.

use super::*;

impl Tui {
    pub(super) fn draw<B: Backend>(&self, terminal: &mut Terminal<B>) -> Result<(), B::Error> {
        terminal.draw(|frame| {
            // Two columns of border and two of padding stand between the text
            // and the edge of the terminal.
            let width = frame.area().width.saturating_sub(6) as usize;
            let (rows_of_text, cursor) = self.composer.rows(width.max(1));
            // The composer takes the rows it needs and no more; what it leaves
            // goes to the answer, which is the thing being read.
            let prompt_height = rows_of_text.len().clamp(1, COMPOSER_ROWS) as u16;

            let rows = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(prompt_height + 2),
                Constraint::Length(1),
            ])
            .split(frame.area());

            frame.render_widget(self.live_rows(rows[0]), rows[0]);

            let frame_style = match self.mode {
                Mode::Approving(..) => Style::default().fg(Color::Yellow),
                Mode::Running => Style::default().fg(Color::Cyan),
                Mode::Idle => Style::default().fg(Color::DarkGray),
            };
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(frame_style)
                .padding(Padding::horizontal(1))
                .title_top(Line::styled(format!("─ {} ", self.title()), frame_style))
                .title_top(Line::styled(self.turn_stats(), frame_style).right_aligned());
            let inner = block.inner(rows[1]);
            frame.render_widget(block, rows[1]);

            // Show the end of a prompt too long for the rows it has: the part
            // being typed is the part that matters.
            let first = rows_of_text.len().saturating_sub(prompt_height as usize);
            frame.render_widget(self.prompt_row(&rows_of_text[first..]), inner);
            frame.render_widget(self.status_row(rows[2]), rows[2]);

            if matches!(self.mode, Mode::Idle) {
                let row = cursor.0.saturating_sub(first) as u16;
                let x = inner.x + 2 + cursor.1 as u16;
                frame.set_cursor_position((
                    x.min(inner.right().saturating_sub(1)),
                    inner.y + row.min(prompt_height.saturating_sub(1)),
                ));
            }
        })?;
        Ok(())
    }

    /// What the frame is for, in its top rule: the model, or the thing being
    /// asked about, since during an approval that is the only question.
    fn title(&self) -> String {
        match &self.mode {
            Mode::Approving(request, _) => format!("approve  {}", request.subject),
            _ => self.model.clone(),
        }
    }

    /// The turn's cost so far, along the top rule where it is legible without
    /// being in the way.
    fn turn_stats(&self) -> String {
        if matches!(self.mode, Mode::Approving(..)) {
            return String::new();
        }
        let mut parts = Vec::new();
        if let Some(at) = self.started {
            parts.push(format!("{}s", at.elapsed().as_secs()));
        }
        if let Some((input, output)) = self.tokens {
            parts.push(format!("{input} in / {output} out"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" {} ", parts.join(" · "))
        }
    }

    /// The unfinished tail of the streaming message, wrapped over the rows the
    /// viewport has spare and scrolled so the newest text is always the last
    /// row visible.
    ///
    /// It is dim and it is markdown-styled, but its markers may still be
    /// half-written — this is the one place a `**` can legitimately be on
    /// screen, and a moment later the finished line lands in scrollback
    /// rendered. Everything above it is already final.
    fn live_rows(&self, area: Rect) -> Paragraph<'_> {
        let width = area.width.saturating_sub(2) as usize;
        let height = area.height.max(1) as usize;
        if self.live.is_empty() || width == 0 {
            return Paragraph::new(Vec::<Line<'static>>::new());
        }
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut lines: Vec<Line<'static>> = markdown::wrap(&self.live, width)
            .into_iter()
            .map(|row| {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(markdown::spans(&row, dim));
                Line::from(spans)
            })
            .collect();
        if lines.len() > height {
            lines.drain(..lines.len() - height);
        }
        Paragraph::new(lines)
    }

    /// The bar under the frame: what is happening on the left, what to press
    /// on the right. "working" alone is the same pixel whether the model is
    /// thinking, running a command, or gone.
    fn status_row(&self, area: Rect) -> Paragraph<'static> {
        let left = match &self.mode {
            Mode::Approving(..) => String::new(),
            _ if self.status.is_empty() => String::new(),
            _ => {
                let turning = match self.started {
                    Some(at) => SPINNER[(at.elapsed().as_millis() / 120) as usize % SPINNER.len()],
                    None => "·",
                };
                format!("{turning} {}", self.status)
            }
        };
        let right = match self.mode {
            Mode::Approving(..) => "the change above is what runs",
            Mode::Running => "ctrl-c or esc to interrupt",
            Mode::Idle if self.composer.text().contains('\n') => "shift-enter for another line",
            Mode::Idle => "",
        };

        let room = area.width.saturating_sub(4) as usize;
        let gap = room
            .saturating_sub(markdown::text_width(&left))
            .saturating_sub(markdown::text_width(right));
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(left, Style::default().fg(Color::Cyan)),
            Span::raw(" ".repeat(gap)),
            Span::styled(
                right.to_owned(),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]))
    }

    fn prompt_row(&self, rows: &[String]) -> Paragraph<'static> {
        let key = |k: &str, what: &str| {
            [
                Span::styled(
                    k.to_owned(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {what}   "),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ]
        };
        match &self.mode {
            Mode::Approving(..) => {
                let mut spans = vec![Span::styled("allow?  ", Style::default().fg(Color::Yellow))];
                spans.extend(key("y", "once"));
                spans.extend(key("a", "this session"));
                spans.extend(key("n", "no"));
                Paragraph::new(Line::from(spans))
            }
            Mode::Running => Paragraph::new(Line::styled(
                "…",
                Style::default().add_modifier(Modifier::DIM),
            )),
            Mode::Idle => Paragraph::new(
                rows.iter()
                    .enumerate()
                    .map(|(i, row)| {
                        Line::from(vec![
                            Span::styled(
                                if i == 0 { "› " } else { "  " },
                                Style::default().fg(Color::Cyan),
                            ),
                            Span::raw(row.clone()),
                        ])
                    })
                    .collect::<Vec<_>>(),
            ),
        }
    }
}
