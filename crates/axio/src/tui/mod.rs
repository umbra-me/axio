//! The interactive surface.
//!
//! An **inline** viewport, not the alternate screen. The finished transcript is
//! printed into the terminal's own scrollback, so it survives the process,
//! scrolls with the scrollbar, and copies with the mouse — a full-screen app
//! owns all of that and gives none of it back. Only the live part — a status
//! line, the composer, the question being asked — lives in the viewport that
//! gets redrawn.
//!
//! Nothing here knows how a turn works. It consumes the same `Event` stream
//! `--json` does and supplies an `Approver`, which is the whole surface
//! contract.

mod approver;
mod markdown;

use std::io::Write;

use axio_core::agent::Agent;
use axio_core::protocol::{
    ApprovalRequest, Decision, Delta, Event, EventKind, ItemBody, NoticeLevel, Preview, ToolStatus,
    TurnOutcome,
};
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_core::Stream;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use approver::{Ask, TuiApprover};

/// Rows the live area occupies. Fixed, because an inline viewport's height is
/// set when it is created and ratatui offers no way to change it after;
/// anything that wants more room is printed into scrollback instead, which is
/// where a diff belongs anyway.
///
/// Three of these are the status, the composer and the hint. The rest hold the
/// tail of the sentence being streamed — one row would show a paragraph as a
/// single scrolling line, which is unreadable, so the tail gets what is left.
const VIEWPORT_ROWS: u16 = 6;

/// What the interface is doing, and therefore what a keystroke means.
enum Mode {
    Idle,
    Running,
    Approving(Box<ApprovalRequest>, tokio::sync::oneshot::Sender<Decision>),
}

pub struct Tui {
    /// Call ids whose terminal status has been printed, so a status reached
    /// through both `ItemUpdated` and `ItemCompleted` prints one line.
    reported: std::collections::HashSet<String>,
    composer: String,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    mode: Mode,
    /// The part of the streaming message that has no newline after it yet, so
    /// the tail can be shown live before it can be rendered and committed.
    live: String,
    /// Markdown state carried across the lines of the message being streamed —
    /// an open code fence is the only thing that outlives a line.
    md: markdown::Renderer,
    /// Whether any of the current message has already reached scrollback, which
    /// decides both what a completed message still has to render and whether a
    /// discarded stream needs to warn that the text above may repeat.
    flushed: bool,
    status: String,
    interrupt_armed: bool,
    model: String,
}

/// Restore the terminal even when the process is dying badly.
///
/// A panic inside a raw-mode program leaves the user with no echo and no line
/// discipline — they have to type `reset` blind. The hook runs before the
/// default one so the message is readable when it arrives.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        previous(info);
    }));
}

fn restore_terminal() -> std::io::Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    let mut out = std::io::stdout();
    crossterm::execute!(out, crossterm::cursor::Show)?;
    out.flush()
}

/// Run the interactive surface until the user leaves.
///
/// `make_agent` builds a fresh agent per turn from the caller's setup; the
/// interface owns no configuration and resolves nothing.
pub async fn run(
    agent: Agent,
    mut events: mpsc::UnboundedReceiver<Event>,
    mut asks: mpsc::UnboundedReceiver<Ask>,
    resumed: bool,
    notices: Vec<axio_core::protocol::Notice>,
    model: String,
) -> std::io::Result<u8> {
    crossterm::terminal::enable_raw_mode()?;
    install_panic_hook();

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_ROWS),
        },
    )?;

    let mut app = Tui {
        reported: std::collections::HashSet::new(),
        composer: String::new(),
        cursor: 0,
        history: Vec::new(),
        history_index: None,
        mode: Mode::Idle,
        live: String::new(),
        md: markdown::Renderer::default(),
        flushed: false,
        status: String::new(),
        interrupt_armed: false,
        model,
    };

    app.banner(&mut terminal, resumed)?;
    let mut agent = Some(agent);
    if let Some(a) = agent.as_mut() {
        a.announce(resumed, notices);
    }

    let mut keys = crossterm::event::EventStream::new();
    let mut turn: Option<tokio::task::JoinHandle<(TurnOutcome, Agent)>> = None;
    let mut cancel = CancellationToken::new();
    let mut leaving = false;

    loop {
        app.draw(&mut terminal)?;

        tokio::select! {
            // Events first: a turn that is producing output should not be
            // starved by a user leaning on a key.
            biased;

            Some(event) = events.recv() => {
                app.on_event(&mut terminal, &event)?;
            }

            Some(ask) = asks.recv() => {
                app.on_ask(&mut terminal, ask)?;
            }

            finished = async { turn.as_mut().expect("guarded").await },
                       if turn.is_some() =>
            {
                turn = None;
                match finished {
                    Ok((outcome, returned)) => {
                        // Drain whatever is still queued before the prompt
                        // returns, so the answer never lands after it.
                        while let Ok(event) = events.try_recv() {
                            app.on_event(&mut terminal, &event)?;
                        }
                        agent = Some(returned);
                        app.on_turn_end(&mut terminal, &outcome)?;
                    }
                    Err(e) => app.push_error(&mut terminal, &format!("the turn panicked: {e}"))?,
                }
                app.mode = Mode::Idle;
                app.status.clear();
                if leaving {
                    break;
                }
            }

            key = next_key(&mut keys) => {
                let Some(key) = key else { break };
                match app.on_key(&mut terminal, key, &cancel)? {
                    Action::None => {}
                    Action::Quit => {
                        if turn.is_some() {
                            // Leave once the turn has unwound, so the session
                            // file gets its terminal record.
                            leaving = true;
                            cancel.cancel();
                        } else {
                            break;
                        }
                    }
                    Action::Submit(prompt) => {
                        // Taken, not cloned: the agent owns the transcript, and
                        // it comes back when the turn ends. Two agents over one
                        // session would each write a different history.
                        let Some(mut running) = agent.take() else {
                            continue;
                        };
                        cancel = CancellationToken::new();
                        app.mode = Mode::Running;
                        app.live.clear();
                        app.status = "working".into();
                        app.push_user(&mut terminal, &prompt)?;

                        let token = cancel.clone();
                        turn = Some(tokio::spawn(async move {
                            let outcome = running.run_turn(prompt, token).await;
                            (outcome, running)
                        }));
                    }
                }
            }
        }
    }

    restore_terminal()?;
    Ok(0)
}

async fn next_key<S>(stream: &mut S) -> Option<KeyEvent>
where
    S: Stream<Item = std::io::Result<TermEvent>> + Unpin,
{
    loop {
        let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await;
        match next {
            // A key release repeats the press on Windows; only presses count.
            Some(Ok(TermEvent::Key(key))) if key.kind == KeyEventKind::Press => return Some(key),
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return None,
        }
    }
}

enum Action {
    None,
    Quit,
    Submit(String),
}

impl Tui {
    // ------------------------------------------------------------------ input

    fn on_key<W: Write>(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        key: KeyEvent,
        cancel: &CancellationToken,
    ) -> std::io::Result<Action> {
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
                    self.cursor = 0;
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

        match key.code {
            KeyCode::Enter => {
                let text = self.composer.trim().to_owned();
                if text.is_empty() {
                    return Ok(Action::None);
                }
                if text == "/exit" || text == "/quit" {
                    return Ok(Action::Quit);
                }
                self.history.push(text.clone());
                self.history_index = None;
                self.composer.clear();
                self.cursor = 0;
                return Ok(Action::Submit(text));
            }
            KeyCode::Char(c) => {
                self.composer.insert(self.byte_at(self.cursor), c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let at = self.byte_at(self.cursor - 1);
                    self.composer.remove(at);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let at = self.byte_at(self.cursor);
                if at < self.composer.len() {
                    self.composer.remove(at);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars(),
            KeyCode::Up => self.recall(-1),
            KeyCode::Down => self.recall(1),
            _ => {}
        }
        Ok(Action::None)
    }

    fn on_approval_key<W: Write>(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        key: KeyEvent,
    ) -> std::io::Result<Action> {
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

    fn recall(&mut self, direction: i32) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.history_index, direction) {
            (None, -1) => Some(self.history.len() - 1),
            (Some(0), -1) => Some(0),
            (Some(i), -1) => Some(i - 1),
            (Some(i), 1) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), 1) => None,
            (None, _) => None,
            _ => self.history_index,
        };
        self.history_index = next;
        self.composer = next.map(|i| self.history[i].clone()).unwrap_or_default();
        self.cursor = self.chars();
    }

    fn chars(&self) -> usize {
        self.composer.chars().count()
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.composer
            .char_indices()
            .nth(char_index)
            .map(|(i, _)| i)
            .unwrap_or(self.composer.len())
    }

    // ----------------------------------------------------------------- events

    fn on_event<W: Write>(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        event: &Event,
    ) -> std::io::Result<()> {
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
                self.live.push_str(text);
                self.flush_lines(terminal)?;
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
                self.status = format!(
                    "working · {} in / {} out",
                    usage.input_tokens, usage.output_tokens
                );
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
    }

    /// Commit every complete line of the streaming message to scrollback,
    /// rendered, leaving the unterminated tail in the viewport.
    ///
    /// A line is the unit because it is the unit markdown is written in: a
    /// heading, a bullet or a paragraph is finished at its newline and can be
    /// rendered without seeing what follows. Holding the whole message back
    /// until it completes would mean watching a blank screen and then having
    /// a page appear at once.
    fn flush_lines<W: Write>(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
    ) -> std::io::Result<()> {
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

    fn on_item<W: Write>(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        item: &axio_core::protocol::Item,
    ) -> std::io::Result<()> {
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
                let remainder = if self.flushed || !self.live.is_empty() {
                    std::mem::take(&mut self.live)
                } else {
                    text.clone()
                };
                let mut lines = if remainder.is_empty() {
                    Vec::new()
                } else {
                    self.md.block(&remainder, width as usize)
                };
                lines.push(Line::raw(""));
                self.begin_message();
                self.push(terminal, lines)?;
            }
            ItemBody::ToolCall {
                subject, status, ..
            } => {
                let line = match status {
                    ToolStatus::Ok { ms, .. } => Some(Line::from(vec![
                        Span::styled("  · ", Style::default().fg(Color::DarkGray)),
                        Span::styled(subject.clone(), Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("  {ms}ms"),
                            Style::default().add_modifier(Modifier::DIM),
                        ),
                    ])),
                    ToolStatus::Failed { message } => Some(Line::from(vec![
                        Span::styled("  ✗ ", Style::default().fg(Color::Red)),
                        Span::raw(subject.clone()),
                        Span::styled(format!("  {message}"), Style::default().fg(Color::Red)),
                    ])),
                    ToolStatus::Denied { message } => Some(Line::from(vec![
                        Span::styled("  ⊘ ", Style::default().fg(Color::Yellow)),
                        Span::raw(subject.clone()),
                        Span::styled(
                            format!("  {}", first_sentence(message)),
                            Style::default().fg(Color::Yellow),
                        ),
                    ])),
                    ToolStatus::Cancelled => Some(Line::styled(
                        format!("  ⊘ {subject}  cancelled"),
                        Style::default().add_modifier(Modifier::DIM),
                    )),
                    _ => None,
                };
                if let Some(line) = line {
                    self.push(terminal, vec![line])?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_ask<W: Write>(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        ask: Ask,
    ) -> std::io::Result<()> {
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

    fn on_turn_end<W: Write>(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        outcome: &TurnOutcome,
    ) -> std::io::Result<()> {
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

    // ---------------------------------------------------------------- drawing

    fn width<W: Write>(&self, terminal: &Terminal<ratatui::backend::CrosstermBackend<W>>) -> u16 {
        terminal.size().map(|s| s.width).unwrap_or(80)
    }

    /// Print into the terminal's own scrollback, above the viewport.
    fn push<W: Write>(
        &self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        lines: Vec<Line<'static>>,
    ) -> std::io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let height = lines.len() as u16;
        terminal.insert_before(height, |buf| {
            Paragraph::new(lines).render(buf.area, buf);
        })
    }

    fn push_user<W: Write>(
        &self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        text: &str,
    ) -> std::io::Result<()> {
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

    fn push_error<W: Write>(
        &self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        message: &str,
    ) -> std::io::Result<()> {
        self.push(
            terminal,
            vec![Line::styled(
                format!("  {message}"),
                Style::default().fg(Color::Red),
            )],
        )
    }

    fn banner<W: Write>(
        &self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
        resumed: bool,
    ) -> std::io::Result<()> {
        let what = if resumed { "resumed" } else { "new session" };
        self.push(
            terminal,
            vec![
                Line::from(vec![
                    Span::styled(
                        "axio ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{}  ·  {what}", self.model),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]),
                Line::styled(
                    "enter to send · ctrl-c to interrupt · ctrl-d to leave",
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Line::raw(""),
            ],
        )
    }

    fn draw<W: Write>(
        &self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<W>>,
    ) -> std::io::Result<()> {
        terminal.draw(|frame| {
            let rows = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(frame.area());

            frame.render_widget(self.live_rows(rows[0]), rows[0]);
            frame.render_widget(self.status_row(), rows[1]);
            frame.render_widget(self.prompt_row(), rows[2]);
            frame.render_widget(self.hint_row(), rows[3]);

            if matches!(self.mode, Mode::Idle) {
                let x = rows[2].x + 2 + self.cursor as u16;
                frame.set_cursor_position((x.min(rows[2].right().saturating_sub(1)), rows[2].y));
            }
        })?;
        Ok(())
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
        let mut lines: Vec<Line<'static>> = wrap(&self.live, width)
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

    fn status_row(&self) -> Paragraph<'_> {
        let text = match &self.mode {
            Mode::Approving(request, _) => format!("  {}", request.subject),
            _ if self.status.is_empty() => String::new(),
            _ => format!("  {}", self.status),
        };
        Paragraph::new(Line::styled(
            text,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
        ))
    }

    fn prompt_row(&self) -> Paragraph<'_> {
        match &self.mode {
            Mode::Approving(..) => Paragraph::new(Line::from(vec![
                Span::styled("  allow? ", Style::default().fg(Color::Cyan)),
                Span::raw("y"),
                Span::styled(" once  ", Style::default().add_modifier(Modifier::DIM)),
                Span::raw("a"),
                Span::styled(
                    " this session  ",
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::raw("n"),
                Span::styled(" no", Style::default().add_modifier(Modifier::DIM)),
            ])),
            Mode::Running => Paragraph::new(Line::styled(
                "  …",
                Style::default().add_modifier(Modifier::DIM),
            )),
            Mode::Idle => Paragraph::new(Line::from(vec![
                Span::styled("› ", Style::default().fg(Color::Cyan)),
                Span::raw(self.composer.clone()),
            ]))
            .wrap(Wrap { trim: false }),
        }
    }

    fn hint_row(&self) -> Paragraph<'_> {
        let text = match self.mode {
            Mode::Approving(..) => "  the change above is what runs",
            Mode::Running => "  ctrl-c or esc to interrupt",
            Mode::Idle => "",
        };
        Paragraph::new(Line::styled(
            text,
            Style::default().add_modifier(Modifier::DIM),
        ))
    }
}

// ------------------------------------------------------------------- helpers

fn first_sentence(text: &str) -> &str {
    match text.find(". ") {
        Some(end) => &text[..=end],
        None => text,
    }
}

/// Wrap on word boundaries, falling back to a hard break for a word that is
/// wider than the terminal.
fn wrap(text: &str, width: usize) -> Vec<String> {
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
            while word.chars().count() > width {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let split: String = word.chars().take(width).collect();
                let taken = split.len();
                out.push(split);
                word = &word[taken..];
            }
            if line.is_empty() {
                line.push_str(word);
            } else if line.chars().count() + 1 + word.chars().count() <= width {
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
fn preview_lines(preview: Option<&Preview>, width: usize) -> Vec<Line<'static>> {
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

pub fn approver() -> (TuiApprover, mpsc::UnboundedReceiver<Ask>) {
    TuiApprover::new()
}

#[cfg(test)]
mod tests {
    use super::*;

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
