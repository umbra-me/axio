//! Turning the agent's events into transcript.
//!
//! The surface consumes the same `Event` stream `--json` does, which is the
//! whole contract: nothing here knows how a turn works, only what it says.

use super::scrollback::{preview_lines, tool_line};
use super::*;

impl Tui {
    pub(super) fn on_event<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        event: &Event,
    ) -> Result<(), B::Error> {
        match &event.kind {
            EventKind::ItemStarted { item } => {
                if matches!(item.body, ItemBody::AgentMessage { .. }) {
                    self.begin_message();
                }
            }
            EventKind::ItemDelta {
                delta: Delta::Text { text },
                ..
            } => {
                self.status = "writing".into();
                self.streamed = true;
                self.live.push_str(text);
                self.flush_lines(terminal)?;
            }
            // Reasoning is never shown — it is the model's working, not its
            // answer — but the fact that it is happening is the difference
            // between a long silence and a broken one.
            EventKind::ItemDelta {
                delta: Delta::Reasoning { .. },
                ..
            } => {
                self.status = "thinking".into();
            }
            EventKind::ItemDiscarded { .. } => {
                // A retry re-sends what was dropped. The unflushed tail is
                // simply dropped, but a line already in scrollback cannot be
                // unprinted — so say so, rather than letting it silently
                // reappear. This is what the one-shot renderer does too.
                let repeats = self.flushed;
                self.begin_message();
                if repeats {
                    self.push(
                        terminal,
                        vec![Line::styled(
                            "  the stream dropped; retrying — the text above may repeat",
                            Style::default().fg(Color::Yellow),
                        )],
                    )?;
                }
                self.status = "retrying".into();
            }
            EventKind::Notice { level, message } => {
                let colour = match level {
                    NoticeLevel::Error => Color::Red,
                    NoticeLevel::Warn => Color::Yellow,
                    NoticeLevel::Info => Color::DarkGray,
                };
                self.push(
                    terminal,
                    vec![Line::styled(
                        format!("  {message}"),
                        Style::default().fg(colour),
                    )],
                )?;
            }
            EventKind::ItemCompleted { item } | EventKind::ItemUpdated { item } => {
                self.on_item(terminal, item)?;
            }
            EventKind::Usage(usage) => {
                self.tokens = Some((usage.input_tokens, usage.output_tokens));
            }
            _ => {}
        }
        Ok(())
    }

    /// Forget whatever was being streamed and start a message from a clean
    /// markdown state.
    fn begin_message(&mut self) {
        self.live.clear();
        self.md = markdown::Renderer::default();
        self.flushed = false;
        self.streamed = false;
    }

    /// Commit every complete line of the streaming message to scrollback,
    /// rendered, leaving the unterminated tail in the viewport.
    ///
    /// A line is the unit because it is the unit markdown is written in: a
    /// heading, a bullet or a paragraph is finished at its newline and can be
    /// rendered without seeing what follows. Holding the whole message back
    /// until it completes would mean watching a blank screen and then having
    /// a page appear at once.
    fn flush_lines<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<(), B::Error> {
        let width = self.width(terminal) as usize;
        let mut lines = Vec::new();
        while let Some(at) = self.live.find('\n') {
            let source: String = self.live.drain(..=at).collect();
            lines.extend(self.md.line(source.trim_end_matches('\n'), width));
        }
        if lines.is_empty() {
            return Ok(());
        }
        self.flushed = true;
        self.push(terminal, lines)
    }

    fn on_item<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        item: &axio_core::protocol::Item,
    ) -> Result<(), B::Error> {
        if let ItemBody::ToolCall {
            call_id, status, ..
        } = &item.body
        {
            let terminal_status = !matches!(
                status,
                ToolStatus::Pending | ToolStatus::AwaitingApproval | ToolStatus::Running
            );
            if terminal_status && !self.reported.insert(call_id.clone()) {
                return Ok(());
            }
        }
        match &item.body {
            ItemBody::AgentMessage { text } if !text.trim().is_empty() => {
                // The deltas of this message are exactly its text, so whatever
                // they already committed must not be rendered again — only the
                // tail they left behind. A provider that streamed nothing
                // leaves both empty, and the whole message is rendered here.
                let width = self.width(terminal);
                let remainder = if self.streamed {
                    std::mem::take(&mut self.live)
                } else {
                    text.clone()
                };
                let mut lines = if remainder.is_empty() {
                    Vec::new()
                } else {
                    self.md.block(&remainder, width as usize)
                };
                // Whatever the renderer is still holding — a table ends at the
                // end of the message as often as it ends at a blank line.
                lines.extend(self.md.finish(width as usize));
                lines.push(Line::raw(""));
                self.begin_message();
                self.push(terminal, lines)?;
            }
            ItemBody::ToolCall {
                subject, status, ..
            } => {
                // Not finished, so nothing to report — but a tool that takes a
                // minute should say which one is taking it.
                if matches!(status, ToolStatus::Running) {
                    self.status = subject.clone();
                    return Ok(());
                }
                let width = self.width(terminal) as usize;
                if let Some(line) = tool_line(subject, status, width) {
                    self.push(terminal, vec![line])?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn on_ask<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        ask: Ask,
    ) -> Result<(), B::Error> {
        // The preview goes to scrollback rather than into the viewport: a diff
        // is the thing the answer depends on, and scrollback is where it can be
        // scrolled back to and copied out of.
        let width = self.width(terminal).saturating_sub(4) as usize;
        let mut lines = vec![
            Line::from(vec![
                Span::styled("  approve  ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    ask.request.subject.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::styled(
                format!("  {}", ask.request.reason),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ];
        lines.extend(preview_lines(ask.request.preview.as_ref(), width));
        self.push(terminal, lines)?;

        self.mode = Mode::Approving(Box::new(ask.request), ask.reply);
        Ok(())
    }

    pub(super) fn on_turn_end<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        outcome: &TurnOutcome,
    ) -> Result<(), B::Error> {
        self.begin_message();
        let line = match outcome {
            TurnOutcome::Completed => None,
            TurnOutcome::Interrupted => Some(("interrupted".to_owned(), Color::Yellow)),
            TurnOutcome::Refused { category, .. } => Some((
                format!(
                    "declined{}",
                    category
                        .as_deref()
                        .map(|c| format!(" ({c})"))
                        .unwrap_or_default()
                ),
                Color::Yellow,
            )),
            TurnOutcome::StepLimit { steps } => {
                Some((format!("stopped after {steps} steps"), Color::Yellow))
            }
            TurnOutcome::BudgetExceeded {
                spent_usd,
                limit_usd,
            } => Some((
                format!("budget exceeded: ${spent_usd:.2} of ${limit_usd:.2}"),
                Color::Yellow,
            )),
            TurnOutcome::Failed { message } => Some((format!("error: {message}"), Color::Red)),
        };
        if let Some((text, colour)) = line {
            self.push(
                terminal,
                vec![
                    Line::styled(format!("  {text}"), Style::default().fg(colour)),
                    Line::raw(""),
                ],
            )?;
        }
        Ok(())
    }
}
