//! Answering an approval: allow it, or refuse it with a note.
//!
//! Two modes and one exit. `Approving` reads a single key; `n` opens the note,
//! which is `Denying`, where the composer takes what is typed. Both leave
//! through `settle`, which records the verdict in scrollback and sends the
//! decision back to the turn that asked.

use super::*;

/// How a typed note is introduced to the model. The same string is what the
/// transcript strips back off to show the note alone.
const NOTE_PREFIX: &str = "denied by the user, who said: ";

impl Tui {
    pub(super) fn on_approval_key<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        key: KeyEvent,
    ) -> Result<Action, B::Error> {
        let decision = match key.code {
            KeyCode::Char('y') | KeyCode::Enter => Decision::Allow,
            KeyCode::Char('a') => Decision::AllowSession,
            // `n` opens the note rather than answering: the refusal is sent
            // when the note is, so the one keystroke that used to end the
            // conversation now starts the useful half of it.
            KeyCode::Char('n') => {
                if let Mode::Approving(request, reply) =
                    std::mem::replace(&mut self.mode, Mode::Running)
                {
                    self.composer.clear();
                    self.mode = Mode::Denying(request, reply);
                }
                return Ok(Action::None);
            }
            KeyCode::Esc => plain_denial(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Decision::Deny {
                feedback: Some("denied by the user".into()),
            },
            _ => return Ok(Action::None),
        };
        self.settle(terminal, decision)
    }

    /// The note under a refusal. Everything that is not the note itself goes
    /// to the composer, which is what makes it a line that can be edited
    /// rather than a string that can only be appended to.
    pub(super) fn on_deny_key<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        key: KeyEvent,
    ) -> Result<Action, B::Error> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.composer.clear();
                return self.settle(terminal, plain_denial());
            }
            KeyCode::Char('c') if ctrl => {
                self.composer.clear();
                return self.settle(
                    terminal,
                    Decision::Deny {
                        feedback: Some("denied by the user".into()),
                    },
                );
            }
            // The composer swallows Enter on an empty line, which is right for
            // a prompt and wrong here: an empty note is an answer.
            KeyCode::Enter
                if self.composer.text().trim().is_empty()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                self.composer.clear();
                return self.settle(terminal, plain_denial());
            }
            _ => {}
        }
        if let Edit::Submit(text) = self.composer.key(key) {
            let note = text.trim();
            let decision = if note.is_empty() {
                plain_denial()
            } else {
                Decision::Deny {
                    feedback: Some(format!(
                        "{NOTE_PREFIX}{note}\n\nDo not retry it as written; follow the note."
                    )),
                }
            };
            return self.settle(terminal, decision);
        }
        Ok(Action::None)
    }

    /// Answer the question the surface is holding and go back to the turn.
    ///
    /// One line in scrollback records the verdict and, for a refusal with a
    /// note, the note — the transcript should show what was said to the model,
    /// not only that something was.
    fn settle<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        decision: Decision,
    ) -> Result<Action, B::Error> {
        let (request, reply) = match std::mem::replace(&mut self.mode, Mode::Running) {
            Mode::Approving(request, reply) | Mode::Denying(request, reply) => (request, reply),
            _ => return Ok(Action::None),
        };
        let (verdict, colour) = match &decision {
            Decision::Allow => ("allowed", Color::Green),
            Decision::AllowSession => ("allowed for this session", Color::Green),
            Decision::Deny { .. } => ("denied", Color::Yellow),
        };
        let mut lines = vec![Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(verdict, Style::default().fg(colour)),
            Span::styled(
                format!("  {}", request.subject),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ])];
        if let Decision::Deny {
            feedback: Some(note),
        } = &decision
            && let Some(said) = note
                .strip_prefix(NOTE_PREFIX)
                .and_then(|rest| rest.split("\n\n").next())
        {
            lines.push(Line::styled(
                format!("    {said}"),
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        self.push(terminal, lines)?;
        let _ = reply.send(decision);
        Ok(Action::None)
    }
}

/// A refusal with nothing more to say. The wording tells the model what to do
/// about it, which is the one thing a bare "no" leaves it to guess.
fn plain_denial() -> Decision {
    Decision::Deny {
        feedback: Some(
            "denied by the user. Do not retry it; continue without it and \
             say what you could not do."
                .into(),
        ),
    }
}
