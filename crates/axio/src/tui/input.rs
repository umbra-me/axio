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
        if let Mode::LoggingIn(..) = self.mode {
            return self.on_login_key(terminal, key);
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

        // The menu owns these keys only while a command name is being typed.
        // Outside that they belong to the composer, where Up recalls history
        // and Tab is a character someone meant to type.
        if commands::choosing(self.composer.text()) {
            let text = self.composer.text().to_owned();
            match key.code {
                KeyCode::Up => {
                    self.menu.step(-1, &text);
                    return Ok(Action::None);
                }
                KeyCode::Down => {
                    self.menu.step(1, &text);
                    return Ok(Action::None);
                }
                // Completion, not selection. Filling the name in leaves the
                // line editable, which is what an argument needs.
                KeyCode::Tab => {
                    if let Some(spec) = self.menu.selection(&text) {
                        self.composer.clear();
                        self.composer.paste(spec.name);
                    }
                    return Ok(Action::None);
                }
                KeyCode::Enter if self.menu.selection(&text).is_some() => {
                    let chosen = self.menu.selection(&text).map(|spec| spec.command);
                    self.composer.clear();
                    self.menu.reset();
                    // Chosen from the menu, so there is no argument: the name
                    // was never finished typing, let alone followed by one.
                    return Ok(chosen.map_or(Action::None, |c| Action::Run(c, String::new())));
                }
                KeyCode::Esc => {
                    self.composer.clear();
                    self.menu.reset();
                    return Ok(Action::None);
                }
                _ => {}
            }
        }

        let edit = self.composer.key(key);
        // Any edit may have changed the filter, and a highlight left where it
        // was would drift onto whatever the shorter list puts underneath it.
        self.menu.reset();

        Ok(match edit {
            // `/exit` is not in the menu: it is kept working for the fingers
            // that already know it, without offering two names for one thing.
            Edit::Submit(text) if text.trim_end() == "/exit" => Action::Quit,
            Edit::Submit(text) => match commands::parse(&text) {
                Some((command, argument)) => Action::Run(command, argument.to_owned()),
                // A mistyped command must not become a prompt. Sending
                // "/quti" to the model spends a turn on a typo and answers a
                // question nobody asked.
                None if commands::choosing(&text) => {
                    self.status = format!("no such command: {}", text.trim_end());
                    Action::None
                }
                None => Action::Submit(text),
            },
            Edit::None => Action::None,
        })
    }

    /// Keys while a credential is being stored.
    ///
    /// Every branch either stays in the flow or leaves it; there is no path
    /// where a typed character reaches the composer, which draws what it holds.
    fn on_login_key<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        key: KeyEvent,
    ) -> Result<Action, B::Error> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Mode::LoggingIn(login) = &mut self.mode else {
            return Ok(Action::None);
        };

        let leave = match key.code {
            KeyCode::Esc => true,
            KeyCode::Char('c') if ctrl => true,
            _ => false,
        };
        if leave {
            self.mode = Mode::Idle;
            self.status = "login cancelled; nothing was stored".into();
            return Ok(Action::None);
        }

        match login.stage() {
            LoginStage::Provider => match key.code {
                KeyCode::Up => login.step_provider(-1),
                KeyCode::Down => login.step_provider(1),
                KeyCode::Enter => login.confirm_provider(),
                _ => {}
            },
            LoginStage::Secret => match key.code {
                KeyCode::Backspace => login.backspace(),
                // Deliberately not `paste`: bracketed paste arrives as its own
                // terminal event, handled where the mode is checked, so a
                // pasted key never lands in the composer either.
                KeyCode::Char(c) if !ctrl => login.push(c),
                KeyCode::Enter => {
                    let Mode::LoggingIn(login) = std::mem::replace(&mut self.mode, Mode::Idle)
                    else {
                        return Ok(Action::None);
                    };
                    let env: Vec<(String, String)> = std::env::vars().collect();
                    let said = match login.save(&crate::paths::axio_home(), &env) {
                        LoginOutcome::Stored(lines) => lines,
                        LoginOutcome::Failed(why) => vec![why],
                    };
                    self.push_command_output(terminal, &said)?;
                }
                _ => {}
            },
        }
        Ok(Action::None)
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
