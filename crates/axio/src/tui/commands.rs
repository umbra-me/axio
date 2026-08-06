//! The slash commands, and the menu that finds them.
//!
//! A command is answered by the surface itself: nothing here reaches the model,
//! spends a token or appends to the transcript. That is what makes the menu
//! cheap enough to open on a keystroke — everything it offers is free.
//!
//! Pure state, so which entries show and which is selected are testable without
//! a terminal.

/// Something the surface can do on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Help,
    Status,
    Model,
    Login,
    New,
    Sessions,
    Clear,
    Quit,
}

pub struct Spec {
    pub name: &'static str,
    pub blurb: &'static str,
    pub command: Command,
}

/// Every command, in the order the menu shows them.
///
/// One list, so the menu, the help text and the dispatch cannot disagree about
/// what exists. A menu entry nothing runs is worse than a command nobody was
/// offered: the first looks like a bug in the command, the second like an
/// absence someone can work around.
pub const COMMANDS: &[Spec] = &[
    Spec {
        name: "/help",
        blurb: "list these commands",
        command: Command::Help,
    },
    Spec {
        name: "/status",
        blurb: "what this session is configured to do",
        command: Command::Status,
    },
    Spec {
        name: "/model",
        blurb: "pick a provider and model, or `/model NAME`",
        command: Command::Model,
    },
    Spec {
        name: "/login",
        blurb: "store a credential, or sign in with a browser",
        command: Command::Login,
    },
    Spec {
        name: "/new",
        blurb: "run a prompt in its own worktree, alongside this session",
        command: Command::New,
    },
    Spec {
        name: "/sessions",
        blurb: "what is running, and what has run",
        command: Command::Sessions,
    },
    Spec {
        name: "/clear",
        blurb: "discard what is in the composer",
        command: Command::Clear,
    },
    Spec {
        name: "/quit",
        blurb: "leave axio",
        command: Command::Quit,
    },
];

/// Which commands a partly-typed name still allows.
pub fn matching(text: &str) -> Vec<&'static Spec> {
    COMMANDS
        .iter()
        .filter(|spec| spec.name.starts_with(text))
        .collect()
}

/// Whether what has been typed is still choosing a command.
///
/// Whitespace ends it. From the first space the words are an argument, and a
/// menu still open would swallow the Enter meant to submit them — which is how
/// a command menu turns into a thing people avoid opening.
pub fn choosing(text: &str) -> bool {
    text.starts_with('/') && !text.contains(char::is_whitespace)
}

/// The command a finished line names, and whatever followed the name.
///
/// Separate from the menu because a line can be submitted without the menu
/// ever opening — pasted, or recalled from history.
///
/// A line whose first word is not a command is prose, argument or not. That
/// keeps `/usr/local/bin holds the binary` a question rather than a rejected
/// command, at the cost of letting a misspelling with an argument through as a
/// prompt; the single-word case, which is nearly always a typo, is caught by
/// the caller instead.
pub fn parse(text: &str) -> Option<(Command, &str)> {
    let text = text.trim();
    let (name, rest) = match text.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (text, ""),
    };
    COMMANDS
        .iter()
        .find(|spec| spec.name == name)
        .map(|spec| (spec.command, rest))
}

/// What the surface should do once a command has run.
///
/// A bool said "leave or do not", which left no room for the one command that
/// finishes somewhere else: listing models is a network round trip, and the
/// loop owns both the spawning and the channel it comes back on.
pub(super) enum After {
    Stay,
    Leave,
}

/// Where the highlight is, and how it moves.
///
/// The index is stored rather than the command, because the list it indexes
/// shrinks as the name is typed. Every read clamps against the current list, so
/// a selection cannot survive into a shorter one and point at nothing.
#[derive(Debug, Default)]
pub struct Menu {
    selected: usize,
}

impl Menu {
    /// The entry the highlight is on, given what is typed now.
    pub fn selection(&self, text: &str) -> Option<&'static Spec> {
        let found = matching(text);
        found
            .get(self.selected.min(found.len().saturating_sub(1)))
            .copied()
    }

    /// The index actually highlighted, for the painter.
    pub fn index(&self, text: &str) -> usize {
        let len = matching(text).len();
        self.selected.min(len.saturating_sub(1))
    }

    /// Move the highlight, wrapping at both ends.
    ///
    /// Wrapping because the list is short: reaching the bottom and pressing
    /// down again should reach the top, not sit there doing nothing.
    pub fn step(&mut self, delta: isize, text: &str) {
        let len = matching(text).len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let at = self.index(text) as isize + delta;
        self.selected = at.rem_euclid(len as isize) as usize;
    }

    /// The slice to draw, and where in the whole list it starts.
    ///
    /// The inline viewport is seven rows and the menu gets what the composer
    /// and the status bar leave — usually three. A longer list therefore
    /// scrolls rather than being cut off at the third entry: the command being
    /// looked for is as likely to be at the bottom as the top, and a menu that
    /// silently omits half of itself is worse than one that makes you scroll.
    pub fn window(&self, text: &str, rows: usize) -> (usize, Vec<&'static Spec>) {
        let found = matching(text);
        if rows == 0 || found.is_empty() {
            return (0, Vec::new());
        }
        let selected = self.index(text);
        let last_start = found.len().saturating_sub(rows);
        let first = selected
            .saturating_sub(rows.saturating_sub(1))
            .min(last_start);
        let end = (first + rows).min(found.len());
        (first, found[first..end].to_vec())
    }

    /// Typing changed the filter, so the highlight goes back to the top.
    ///
    /// Keeping it where it was means a keystroke that shortens the list moves
    /// the highlight onto a different command without the user asking, and the
    /// next Enter runs whatever landed under it.
    pub fn reset(&mut self) {
        self.selected = 0;
    }
}

impl super::Tui {
    /// Run a command. Returns whether the surface should leave.
    ///
    /// Every arm is answered here and now — no arm sends a prompt, spends a
    /// token or touches the transcript, which is what lets a command run while
    /// a turn is still going.
    pub(super) fn run_command<B: super::Backend>(
        &mut self,
        terminal: &mut super::Terminal<B>,
        command: Command,
        argument: &str,
        agent: Option<&mut axio_core::agent::Agent>,
    ) -> Result<After, B::Error> {
        match command {
            Command::Quit => return Ok(After::Leave),
            Command::Status => {
                let mut said = vec![format!("{:<11}{}", "model", self.model)];
                said.extend(self.facts.iter().cloned());
                self.push_command_output(terminal, &said)?;
            }
            Command::Model => return self.change_model(terminal, argument, agent),
            Command::New => {
                let Some(supervisor) = self.supervisor.clone() else {
                    self.push_command_output(
                        terminal,
                        &[
                            "sessions are unavailable here (the index could not be opened)"
                                .to_owned(),
                        ],
                    )?;
                    return Ok(After::Stay);
                };
                let prompt = argument.trim();
                if prompt.is_empty() {
                    // A worktree cut for no prompt is litter someone has to find
                    // and remove later, so this asks rather than guessing.
                    self.push_command_output(
                        terminal,
                        &["/new needs a prompt — `/new fix the failing test`".to_owned()],
                    )?;
                    return Ok(After::Stay);
                }
                super::background::spawn(
                    supervisor,
                    prompt.to_owned(),
                    self.notes.clone(),
                    self.repo.clone(),
                );
                self.push_command_output(
                    terminal,
                    &[format!("queued in its own worktree: {prompt}")],
                )?;
            }
            Command::Sessions => {
                let said = match self.supervisor.as_deref() {
                    Some(supervisor) => super::background::summary(supervisor),
                    None => vec![
                        "sessions are unavailable here (the index could not be opened)".to_owned(),
                    ],
                };
                self.push_command_output(terminal, &said)?;
            }
            Command::Clear => {
                self.composer.clear();
                self.status = "composer cleared".into();
            }
            Command::Help => {
                let width = COMMANDS
                    .iter()
                    .map(|spec| spec.name.len())
                    .max()
                    .unwrap_or(0);
                let said: Vec<String> = COMMANDS
                    .iter()
                    .map(|spec| format!("{:<width$}  {}", spec.name, spec.blurb))
                    .collect();
                self.push_command_output(terminal, &said)?;
            }
            Command::Login => {
                // Announced in scrollback before the mode changes, because the
                // viewport is about to be a form and the reason it appeared
                // should outlive it.
                self.push_command_output(
                    terminal,
                    &["storing a credential — esc to leave without saving".to_owned()],
                )?;
                self.mode = super::Mode::LoggingIn(super::Login::default());
                self.status.clear();
            }
        }
        Ok(After::Stay)
    }

    /// Show the model, or change it.
    ///
    /// Its own function because it is the only command that reaches the agent,
    /// and the only one that can be refused for a reason that has nothing to do
    /// with what was typed.
    fn change_model<B: super::Backend>(
        &mut self,
        terminal: &mut super::Terminal<B>,
        argument: &str,
        agent: Option<&mut axio_core::agent::Agent>,
    ) -> Result<After, B::Error> {
        // While a turn runs the agent has been moved into it, and it comes back
        // when the turn ends. Changing the model underneath a request in flight
        // is not something to arrange; waiting is.
        let Some(agent) = agent else {
            self.status = "a turn is running — the model can change once it finishes".into();
            return Ok(After::Stay);
        };

        // Bare `/model` opens the provider stage. Provider and model are
        // chosen in one flow because changing the provider alone leaves the
        // session holding a model its new endpoint has never heard of — a
        // state two separate commands would make reachable.
        if argument.is_empty() {
            self.pending_provider = None;
            let offers = self.offers.clone();
            let current = agent.id_of_provider().to_owned();
            self.mode = super::Mode::PickingModel(super::Picker::providers(offers, &current));
            return Ok(After::Stay);
        }

        let previous = agent.model().to_owned();
        let was = agent.id_of_provider().to_owned();
        // Applied together, or not at all. A provider that changed while the
        // model did not is a session pointed at an endpoint that has never
        // heard of what it is about to ask for.
        let moving = match self.pending_provider.take() {
            Some(name) if name != was => Some(name),
            _ => None,
        };

        if moving.is_none() && argument == previous {
            self.status = format!("already using {previous}");
            return Ok(After::Stay);
        }

        if let Some(name) = &moving {
            match (self.factory)(name) {
                Ok(provider) => agent.adopt(provider, argument),
                Err(why) => {
                    self.status = format!("staying on {was}: {why}");
                    return Ok(After::Stay);
                }
            }
        } else {
            agent.set_model(argument);
        }
        // The frame's title reads this, so leaving it stale would put the old
        // name above every answer the new model gives.
        self.model = argument.to_owned();

        // Remembered, not written. The next turn is what decides whether this
        // becomes the default; a name is not checked until then.
        self.unproven_default = Some((agent.id_of_provider().to_owned(), argument.to_owned()));

        let mut said = vec![format!("model      {previous} → {argument}")];
        if agent.model() != agent.session_model() {
            said.push(format!(
                "  reasoning recorded under `{}` will not be replayed",
                agent.session_model()
            ));
        }
        // Nothing validates the name: the provider is not asked until the next
        // request, so a typo surfaces then rather than now. Saying so is
        // cheaper than a lookup that would need a round trip to be honest.
        said.push("  the name is not checked until the next request".into());
        self.push_command_output(terminal, &said)?;
        Ok(After::Stay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_slash_offers_everything() {
        assert_eq!(matching("/").len(), COMMANDS.len());
    }

    #[test]
    fn typing_narrows_the_list() {
        let found = matching("/lo");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].command, Command::Login);
    }

    #[test]
    fn a_name_nothing_matches_offers_nothing() {
        assert!(matching("/nonsense").is_empty());
    }

    /// The menu has to let go once an argument is being typed, or it eats the
    /// Enter that submits it.
    #[test]
    fn whitespace_ends_the_choosing() {
        assert!(choosing("/lo"));
        assert!(!choosing("/login "));
        assert!(!choosing("write me a poem"));
        assert!(!choosing(""));
        // A slash inside ordinary prose is not a command being chosen.
        assert!(!choosing("what does a/b mean"));
    }

    #[test]
    fn a_submitted_name_resolves_without_the_menu() {
        assert_eq!(parse("/quit"), Some((Command::Quit, "")));
        assert_eq!(parse("/quit  "), Some((Command::Quit, "")));
        assert_eq!(parse("/qui"), None, "a prefix is not a command");
        assert_eq!(parse("tell me about /help"), None);
    }

    #[test]
    fn an_argument_survives_the_parse() {
        assert_eq!(
            parse("/model kimi-k2.7-code"),
            Some((Command::Model, "kimi-k2.7-code"))
        );
        // Whitespace around it is the terminal's, not the user's intent.
        assert_eq!(parse("/model   spaced  "), Some((Command::Model, "spaced")));
        assert_eq!(parse("/model"), Some((Command::Model, "")));
    }

    /// Prose that opens with a path must stay prose. The single-word case is
    /// the one worth refusing, and the caller does that.
    #[test]
    fn a_line_opening_with_a_path_is_not_a_command() {
        assert_eq!(parse("/usr/local/bin holds the binary"), None);
    }

    #[test]
    fn the_highlight_wraps_at_both_ends() {
        let mut menu = Menu::default();
        assert_eq!(menu.index("/"), 0);
        menu.step(-1, "/");
        assert_eq!(menu.index("/"), COMMANDS.len() - 1, "up from the top wraps");
        menu.step(1, "/");
        assert_eq!(menu.index("/"), 0);
    }

    /// The regression this guards: the index outliving the list it indexes.
    /// Select the last of four, type a letter that leaves one match, and an
    /// unclamped index points past the end — so the menu shows nothing
    /// highlighted and Enter runs nothing, with no way to tell why.
    #[test]
    fn a_selection_cannot_survive_into_a_shorter_list() {
        let mut menu = Menu::default();
        menu.step(-1, "/");
        assert_eq!(menu.index("/"), COMMANDS.len() - 1);

        assert_eq!(menu.index("/lo"), 0, "clamped to the only match");
        assert_eq!(
            menu.selection("/lo").map(|s| s.command),
            Some(Command::Login)
        );
        assert!(menu.selection("/nonsense").is_none());
    }

    /// Three rows for a longer list means the highlight has to drag the window
    /// with it, or moving down past the third entry highlights something the
    /// user cannot see.
    #[test]
    fn the_window_follows_the_highlight() {
        let mut menu = Menu::default();
        let rows = 3;

        let (first, shown) = menu.window("/", rows);
        assert_eq!(first, 0);
        assert_eq!(shown.len(), rows.min(COMMANDS.len()));

        // Walk to the last entry; the window must have moved to contain it.
        for _ in 0..COMMANDS.len() - 1 {
            menu.step(1, "/");
        }
        let (first, shown) = menu.window("/", rows);
        assert_eq!(
            first + shown.len(),
            COMMANDS.len(),
            "the last entry must be on screen"
        );
        assert_eq!(
            shown.last().map(|s| s.command),
            COMMANDS.last().map(|s| s.command)
        );
    }

    #[test]
    fn a_window_with_no_room_shows_nothing_rather_than_panicking() {
        let menu = Menu::default();
        assert!(menu.window("/", 0).1.is_empty());
        assert!(menu.window("/nonsense", 3).1.is_empty());
    }

    #[test]
    fn every_command_is_reachable_by_its_own_name() {
        // A command the menu lists but `parse` cannot resolve would run from
        // the menu and do nothing when typed in full.
        for spec in COMMANDS {
            assert_eq!(
                parse(spec.name).map(|(command, _)| command),
                Some(spec.command),
                "{} does not resolve",
                spec.name
            );
            assert!(spec.name.starts_with('/'), "{} needs a slash", spec.name);
        }
    }
}
