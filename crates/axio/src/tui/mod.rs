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
mod commands;
mod composer;
mod events;
mod frame;
mod highlight;
mod input;
mod login;
mod markdown;
mod overlay;
mod paint;
mod scrollback;

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
use commands::{Command, Menu};
use composer::{Composer, Edit};
use login::{Login, Outcome as LoginOutcome, Stage as LoginStage};

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

/// Below this many rows the composer's frame is dropped rather than the line
/// being typed. Three rows of frame plus a status bar in a five-row terminal
/// leaves nothing to type into, and a prompt you cannot see is worse than a
/// prompt without a border.
const FRAMED_ROWS: u16 = 6;

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
    /// Storing a credential. Its own mode rather than a flag, because while it
    /// is on every keystroke belongs to it — a character typed here must not
    /// also reach the composer, where it would be drawn.
    LoggingIn(Login),
}

pub struct Tui {
    /// Call ids whose terminal status has been printed, so a status reached
    /// through both `ItemUpdated` and `ItemCompleted` prints one line.
    reported: std::collections::HashSet<String>,
    composer: Composer,
    /// Where the highlight is in the slash menu. Kept across openings so the
    /// menu is not the only widget on screen with amnesia; the filter clamps
    /// it, so a stale index can never select something that is not shown.
    menu: Menu,
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
    /// What `/status` reports besides the model: provider, endpoint, where the
    /// credential came from, the permission rules, the workspace root.
    ///
    /// Rendered once, before the surface starts, because none of it can change
    /// while a session runs — the model is the one part that can, and it is
    /// read live from the field above rather than kept here.
    facts: Vec<String>,
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
    facts: Vec<String>,
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
        facts,
        reported: std::collections::HashSet::new(),
        composer: Composer::default(),
        menu: Menu::default(),
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
                        match &mut app.mode {
                            Mode::Idle => app.composer.paste(&text),
                            // Pasting is how a credential normally arrives, so
                            // routing it to the composer here would print the
                            // key on screen — the one thing this flow exists
                            // to prevent.
                            Mode::LoggingIn(login)
                                if login.stage() == LoginStage::Secret =>
                            {
                                login.paste(&text)
                            }
                            _ => {}
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
                    Action::Run(command, argument) => {
                        if app.run_command(
                            &mut terminal,
                            command,
                            &argument,
                            agent.as_mut(),
                        )? {
                            if turn.is_some() {
                                leaving = true;
                                cancel.cancel();
                            } else {
                                break;
                            }
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
    /// A slash command. Separate from `Submit` because it never becomes a
    /// turn: no prompt is sent, nothing is recorded, and the agent is not
    /// taken — so a command works during a turn as readily as between them.
    Run(Command, String),
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
            facts: Vec::new(),
            reported: std::collections::HashSet::new(),
            composer: Composer::default(),
            menu: Menu::default(),
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
}
