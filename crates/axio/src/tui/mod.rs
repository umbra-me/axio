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
mod composer;
mod frame;
mod highlight;
mod markdown;

use std::io::Write;
use std::time::Instant;

use axio_core::agent::Agent;
use axio_core::protocol::{
    ApprovalRequest, Decision, Delta, Event, EventKind, ItemBody, NoticeLevel, Preview, ToolStatus,
    TurnOutcome,
};
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_core::Stream;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use approver::{Ask, TuiApprover};
use composer::{Composer, Edit};

/// Rows the live area occupies. Fixed, because an inline viewport's height is
/// set when it is created and ratatui offers no way to change it after;
/// anything that wants more room is printed into scrollback instead, which is
/// where a diff belongs anyway.
///
/// Four of these are the composer's frame, the line inside it and the status
/// bar under it. The rest hold the tail of the sentence being streamed — one
/// row would show a paragraph as a single scrolling line, which is unreadable,
/// so the tail gets what is left.
const VIEWPORT_ROWS: u16 = 7;

/// Rows the composer may grow to before it scrolls instead. A multi-line prompt
/// is common enough to deserve room and rare enough not to deserve the whole
/// viewport, which the answer is still streaming into.
const COMPOSER_ROWS: usize = 3;

/// Turned while a turn runs, off the clock rather than off a counter, so it
/// keeps time whether the model is flooding the surface or saying nothing.
const SPINNER: [&str; 4] = ["·", "∙", "•", "∙"];

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
    composer: Composer,
    mode: Mode,
    /// The part of the streaming message that has no newline after it yet, so
    /// the tail can be shown live before it can be rendered and committed.
    live: String,
    /// Markdown state carried across the lines of the message being streamed —
    /// an open code fence is the only thing that outlives a line.
    md: markdown::Renderer,
    /// Whether any of the current message has already reached scrollback, which
    /// is what decides whether a discarded stream has to warn that the text
    /// above may repeat.
    flushed: bool,
    /// Whether the renderer has already been fed this message's deltas. It is
    /// not the same question as `flushed`: a message whose lines are all still
    /// held back — a table, whose columns are only known at its last row — has
    /// been consumed without anything reaching the screen, and rendering the
    /// completed item again would draw it twice.
    streamed: bool,
    /// What the turn is doing right now — thinking, writing, or the subject of
    /// the tool it is waiting on.
    status: String,
    /// The turn's cumulative usage, kept apart from `status` so a token count
    /// arriving does not overwrite what the turn was doing.
    tokens: Option<(u64, u64)>,
    interrupt_armed: bool,
    model: String,
    /// When the running turn started, which is what the status counts up from.
    /// A turn that has produced nothing for thirty seconds looks identical to a
    /// hung one unless something on screen is still moving.
    started: Option<Instant>,
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
    crossterm::execute!(
        out,
        crossterm::event::DisableBracketedPaste,
        crossterm::cursor::Show
    )?;
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
    // Without this a pasted paragraph arrives as keystrokes, and its first
    // newline submits it — the rest of the paste then types itself into
    // whatever the surface does next.
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_ROWS),
        },
    )?;

    let mut app = Tui {
        reported: std::collections::HashSet::new(),
        composer: Composer::default(),
        mode: Mode::Idle,
        live: String::new(),
        md: markdown::Renderer::default(),
        flushed: false,
        streamed: false,
        status: String::new(),
        tokens: None,
        interrupt_armed: false,
        model,
        started: None,
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
    let mut clock = frame::Clock::new(Instant::now());

    loop {
        // Painting is paced rather than driven: a fast stream marks the surface
        // dirty far more often than a terminal can usefully repaint, and every
        // wasted frame is bandwidth the streaming text is not getting.
        if clock.due(Instant::now()) {
            app.draw(&mut terminal)?;
            clock.drew(Instant::now());
        }

        tokio::select! {
            // Events first: a turn that is producing output should not be
            // starved by a user leaning on a key.
            biased;

            // Whatever is pending, painted as soon as a frame has passed.
            _ = tokio::time::sleep_until(clock.deadline().into()), if clock.pending() => {}

            // While a turn runs the status keeps time, so a model that has gone
            // quiet still looks different from one that has hung.
            _ = tokio::time::sleep(frame::FRAME * 8), if app.started.is_some() => {
                clock.mark();
            }

            Some(event) = events.recv() => {
                app.on_event(&mut terminal, &event)?;
                clock.mark();
            }

            Some(ask) = asks.recv() => {
                app.on_ask(&mut terminal, ask)?;
                clock.mark();
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
                app.tokens = None;
                app.started = None;
                clock.mark();
                if leaving {
                    break;
                }
            }

            input = next_input(&mut keys) => {
                let Some(input) = input else { break };
                clock.mark();
                let key = match input {
                    // A resize is not something to handle so much as something
                    // to notice: the next draw re-measures, and the rows that
                    // already reached scrollback belong to the terminal now.
                    TermEvent::Resize(..) => continue,
                    TermEvent::Paste(text) => {
                        if matches!(app.mode, Mode::Idle) {
                            app.composer.paste(&text);
                        }
                        continue;
                    }
                    TermEvent::Key(key) => key,
                    _ => continue,
                };
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
                        app.started = Some(Instant::now());
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

/// The next terminal event the surface has any use for.
///
/// Key releases are dropped here rather than downstream: on Windows every press
/// is reported twice, and one character typed would become two everywhere that
/// handles a key.
async fn next_input<S>(stream: &mut S) -> Option<TermEvent>
where
    S: Stream<Item = std::io::Result<TermEvent>> + Unpin,
{
    loop {
        let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await;
        match next {
            Some(Ok(TermEvent::Key(key))) if key.kind != KeyEventKind::Press => continue,
            Some(Ok(event)) => return Some(event),
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

    fn on_key<B: Backend>(
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

    // ----------------------------------------------------------------- events

    fn on_event<B: Backend>(
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

    fn on_ask<B: Backend>(&mut self, terminal: &mut Terminal<B>, ask: Ask) -> Result<(), B::Error> {
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

    fn on_turn_end<B: Backend>(
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

    // ---------------------------------------------------------------- drawing

    fn width<B: Backend>(&self, terminal: &Terminal<B>) -> u16 {
        terminal.size().map(|s| s.width).unwrap_or(80)
    }

    /// Print into the terminal's own scrollback, above the viewport.
    fn push<B: Backend>(
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

    fn push_user<B: Backend>(
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

    fn push_error<B: Backend>(
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

    fn banner<B: Backend>(
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

    fn draw<B: Backend>(&self, terminal: &mut Terminal<B>) -> Result<(), B::Error> {
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

// ------------------------------------------------------------------- helpers

/// One finished tool call, as a row: a mark carrying the outcome, the tool's
/// name in its own column so a run of calls reads as a list rather than as
/// prose, what it acted on, and how long it took against the right margin.
///
/// The subject is `name:detail` — the same canonical string the permission
/// engine decided on, so what is shown is what was authorised.
fn tool_line(subject: &str, status: &ToolStatus, width: usize) -> Option<Line<'static>> {
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
    use axio_core::protocol::{Item, ItemId, SessionId};
    use ratatui::backend::TestBackend;

    /// A surface over a fake terminal, which is what makes any of this
    /// assertable: everything below the run loop is generic over the backend
    /// precisely so a test can hold one.
    fn surface(width: u16, height: u16) -> (Tui, Terminal<TestBackend>) {
        let mut backend = TestBackend::new(width, height);
        // Anchored where a real one is: below whatever the shell already
        // printed, with room above it for the transcript to land in.
        backend
            .set_cursor_position(ratatui::layout::Position::new(0, height - VIEWPORT_ROWS))
            .expect("a cursor");
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(VIEWPORT_ROWS),
            },
        )
        .expect("a terminal");
        let app = Tui {
            reported: std::collections::HashSet::new(),
            composer: Composer::default(),
            mode: Mode::Idle,
            live: String::new(),
            md: markdown::Renderer::default(),
            flushed: false,
            streamed: false,
            status: String::new(),
            tokens: None,
            interrupt_armed: false,
            model: "test-model".into(),
            started: None,
        };
        (app, terminal)
    }

    fn rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// Everything the terminal has been told, whether it is still on screen or
    /// has scrolled off it.
    fn everything(terminal: &Terminal<TestBackend>) -> String {
        let mut lines = rows(terminal.backend().scrollback());
        lines.extend(rows(terminal.backend().buffer()));
        lines.join("\n")
    }

    fn event(kind: EventKind) -> Event {
        Event {
            seq: 1,
            session: SessionId::nil(),
            turn: None,
            at_ms: 0,
            kind,
        }
    }

    fn delta(text: &str) -> Event {
        event(EventKind::ItemDelta {
            id: ItemId::nil(),
            delta: Delta::Text { text: text.into() },
        })
    }

    fn completed(text: &str) -> Event {
        event(EventKind::ItemCompleted {
            item: Item {
                id: ItemId::nil(),
                body: ItemBody::AgentMessage { text: text.into() },
            },
        })
    }

    #[test]
    fn a_finished_line_leaves_the_viewport_and_the_tail_stays_in_it() {
        // The shape the whole surface rests on: what is settled belongs to the
        // terminal, and only what is still arriving is redrawn.
        let (mut app, mut terminal) = surface(40, 12);
        app.on_event(&mut terminal, &delta("**done** with this\nstill ty"))
            .expect("handled");
        app.draw(&mut terminal).expect("drawn");

        let visible = everything(&terminal);
        assert!(visible.contains("done with this"), "{visible}");
        assert!(visible.contains("still ty"), "{visible}");

        // The committed line is in the terminal's own history, not in the four
        // rows the surface owns; the unfinished tail is the other way round.
        let viewport = rows(terminal.backend().buffer())
            .split_off((12 - VIEWPORT_ROWS) as usize)
            .join("\n");
        assert!(!viewport.contains("done with this"), "{viewport}");
        assert!(viewport.contains("still ty"), "{viewport}");
    }

    #[test]
    fn a_streamed_message_is_not_printed_again_when_it_completes() {
        // Deltas commit line by line and the completed item carries the whole
        // message: rendering both would print every finished line twice.
        let (mut app, mut terminal) = surface(40, 12);
        app.on_event(&mut terminal, &delta("alpha\nbeta"))
            .expect("handled");
        app.on_event(&mut terminal, &completed("alpha\nbeta"))
            .expect("handled");
        app.draw(&mut terminal).expect("drawn");

        let visible = everything(&terminal);
        assert_eq!(visible.matches("alpha").count(), 1, "{visible}");
        assert_eq!(visible.matches("beta").count(), 1, "{visible}");
    }

    #[test]
    fn a_table_that_runs_to_the_end_of_a_message_is_drawn_once() {
        // The trap the `streamed` flag exists for: a table is held back until
        // its last row, so nothing has reached the screen when the message
        // completes — and a surface that took that for "nothing was streamed"
        // would render the whole message again on top of the held-back table.
        let (mut app, mut terminal) = surface(50, 14);
        let table = "| Fruit | Cost |\n|---|---|\n| apple | 3 |";
        app.on_event(&mut terminal, &delta(table)).expect("handled");
        app.on_event(&mut terminal, &completed(table))
            .expect("handled");

        let visible = everything(&terminal);
        assert_eq!(visible.matches("apple").count(), 1, "{visible}");
        assert!(!visible.contains('|'), "{visible}");
    }

    #[test]
    fn a_message_that_never_streamed_is_rendered_whole() {
        let (mut app, mut terminal) = surface(40, 12);
        app.on_event(&mut terminal, &completed("# Heading\n\nplain words"))
            .expect("handled");
        let visible = everything(&terminal);
        assert!(visible.contains("Heading"), "{visible}");
        assert!(!visible.contains('#'), "{visible}");
        assert!(visible.contains("plain words"), "{visible}");
    }

    #[test]
    fn a_dropped_stream_admits_that_what_it_printed_may_come_back() {
        let (mut app, mut terminal) = surface(60, 12);
        app.on_event(&mut terminal, &delta("half an answer\n"))
            .expect("handled");
        app.on_event(
            &mut terminal,
            &event(EventKind::ItemDiscarded {
                id: ItemId::nil(),
                reason: "overloaded".into(),
            }),
        )
        .expect("handled");
        assert!(everything(&terminal).contains("may repeat"));
    }

    #[test]
    fn nothing_is_said_about_a_repeat_that_cannot_have_happened() {
        // Only the tail was dropped, so there is nothing on screen to repeat
        // and no reason to worry the user about one.
        let (mut app, mut terminal) = surface(60, 12);
        app.on_event(&mut terminal, &delta("no newline yet"))
            .expect("handled");
        app.on_event(
            &mut terminal,
            &event(EventKind::ItemDiscarded {
                id: ItemId::nil(),
                reason: "overloaded".into(),
            }),
        )
        .expect("handled");
        assert!(!everything(&terminal).contains("may repeat"));
    }

    #[test]
    fn the_composer_grows_for_a_multi_line_prompt_and_stops_growing() {
        let (mut app, mut terminal) = surface(40, 12);
        app.composer.paste("one\ntwo");
        app.draw(&mut terminal).expect("drawn");
        let visible = rows(terminal.backend().buffer()).join("\n");
        assert!(visible.contains("› one"), "{visible}");
        assert!(visible.contains("  two"), "{visible}");

        // More lines than it has rows: the end is what is being typed, so the
        // end is what stays on screen.
        app.composer.paste("\nthree\nfour");
        app.draw(&mut terminal).expect("drawn");
        let visible = rows(terminal.backend().buffer()).join("\n");
        assert!(visible.contains("four"), "{visible}");
        assert!(!visible.contains("one"), "{visible}");
    }

    #[test]
    fn a_tool_result_reports_once_however_many_events_carry_it() {
        let (mut app, mut terminal) = surface(50, 12);
        let ok = || ToolStatus::Ok {
            output: "hi".into(),
            truncated: false,
            spill: None,
            ms: 3,
        };
        let call = |status| {
            event(EventKind::ItemUpdated {
                item: Item {
                    id: ItemId::nil(),
                    body: ItemBody::ToolCall {
                        call_id: "call_1".into(),
                        name: "read".into(),
                        input: serde_json::json!({}),
                        subject: "read:notes.md".into(),
                        preview: None,
                        status,
                    },
                },
            })
        };
        app.on_event(&mut terminal, &call(ok())).expect("handled");
        app.on_event(&mut terminal, &call(ok())).expect("handled");
        let visible = everything(&terminal);
        assert_eq!(visible.matches("notes.md").count(), 1, "{visible}");
    }

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
