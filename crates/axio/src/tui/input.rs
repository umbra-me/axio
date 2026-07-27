//! What a keystroke means, which depends on what the surface is doing.
//!
//! The modal keys live here — interrupt, leave, answer an approval. Everything
//! that only means something to text is the composer's.

use super::*;

impl Tui {
    pub(super) fn on_key<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        key: KeyEvent,
        cancel: &CancellationToken,
    ) -> Result<Action, B::Error> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if let Mode::Approving(..) = self.mode {
            return self.on_approval_key(terminal, key);
        }

        match key.code {
            KeyCode::Char('c') if ctrl => {
                if matches!(self.mode, Mode::Running) {
                    cancel.cancel();
                    self.status = "interrupting".into();
                    return Ok(Action::None);
                }
                if self.composer.is_empty() {
                    if self.interrupt_armed {
                        return Ok(Action::Quit);
                    }
                    self.interrupt_armed = true;
                    self.status = "press ctrl-c again to exit".into();
                } else {
                    self.composer.clear();
                }
                return Ok(Action::None);
            }
            KeyCode::Char('d') if ctrl && self.composer.is_empty() => return Ok(Action::Quit),
            KeyCode::Esc if matches!(self.mode, Mode::Running) => {
                cancel.cancel();
                self.status = "interrupting".into();
                return Ok(Action::None);
            }
            _ => {}
        }
        self.interrupt_armed = false;

        // While a turn runs the composer is not accepting work: a second turn
        // would race the first for the same transcript.
        if matches!(self.mode, Mode::Running) {
            return Ok(Action::None);
        }

        Ok(match self.composer.key(key) {
            Edit::Submit(text) if text == "/exit" || text == "/quit" => Action::Quit,
            Edit::Submit(text) => Action::Submit(text),
            Edit::None => Action::None,
        })
    }

    fn on_approval_key<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        key: KeyEvent,
    ) -> Result<Action, B::Error> {
        let decision = match key.code {
            KeyCode::Char('y') | KeyCode::Enter => Decision::Allow,
            KeyCode::Char('a') => Decision::AllowSession,
            KeyCode::Char('n') | KeyCode::Esc => Decision::Deny {
                feedback: Some(
                    "denied by the user. Do not retry it; continue without it and \
                     say what you could not do."
                        .into(),
                ),
            },
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Decision::Deny {
                feedback: Some("denied by the user".into()),
            },
            _ => return Ok(Action::None),
        };

        let mode = std::mem::replace(&mut self.mode, Mode::Running);
        if let Mode::Approving(request, reply) = mode {
            let verdict = match &decision {
                Decision::Allow => "allowed",
                Decision::AllowSession => "allowed for this session",
                Decision::Deny { .. } => "denied",
            };
            self.push(
                terminal,
                vec![Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(
                        verdict,
                        Style::default().fg(match decision {
                            Decision::Deny { .. } => Color::Yellow,
                            _ => Color::Green,
                        }),
                    ),
                    Span::styled(
                        format!("  {}", request.subject),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ])],
            )?;
            let _ = reply.send(decision);
        }
        Ok(Action::None)
    }
}
