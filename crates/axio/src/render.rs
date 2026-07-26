//! Renderers: two implementations of "consume events, write bytes".
//!
//! Neither knows anything about the loop, and the loop knows nothing about
//! either. `--json` is a second renderer, never a second loop — which is the
//! whole reason the protocol is serde-derived even though nothing crosses a
//! process boundary yet.

use std::io::Write;

use axio_core::protocol::{Delta, Event, EventKind, NoticeLevel, TurnOutcome};

/// Colour is decided once, from whether the sink is a terminal, and never from
/// whether stdin is. `axio -p x > out.txt` run from a terminal must still write
/// zero escape bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub colour: bool,
}

impl Style {
    /// No escape bytes at all — the correct choice for any non-terminal sink.
    pub const PLAIN: Style = Style { colour: false };

    const DIM: &'static str = "\x1b[2m";
    const YELLOW: &'static str = "\x1b[33m";
    const RED: &'static str = "\x1b[31m";
    const RESET: &'static str = "\x1b[0m";

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.colour {
            format!("{code}{text}{}", Self::RESET)
        } else {
            text.to_owned()
        }
    }
}

pub trait Renderer {
    fn handle(&mut self, event: &Event) -> std::io::Result<()>;
    fn finish(&mut self) -> std::io::Result<()>;
}

/// Human-readable output for the one-shot CLI.
///
/// Reasoning is not printed: on this model the text is a summary at best and
/// empty by default, and a one-shot caller piping to a file wants the answer.
pub struct PlainRenderer<W: Write> {
    out: W,
    err: Box<dyn Write + Send>,
    style: Style,
    /// Set when a partial item is dropped, so the retry's text is not appended
    /// to text the reader has already seen.
    discarded: bool,
    wrote_any: bool,
}

impl<W: Write> PlainRenderer<W> {
    pub fn new(out: W, err: Box<dyn Write + Send>, style: Style) -> Self {
        Self {
            out,
            err,
            style,
            discarded: false,
            wrote_any: false,
        }
    }
}

impl<W: Write> Renderer for PlainRenderer<W> {
    fn handle(&mut self, event: &Event) -> std::io::Result<()> {
        match &event.kind {
            EventKind::ItemDelta {
                delta: Delta::Text { text },
                ..
            } => {
                self.wrote_any = true;
                write!(self.out, "{text}")?;
                // Streamed output is useless if it arrives in one block at exit.
                self.out.flush()?;
            }
            EventKind::ItemDiscarded { .. } => {
                // Exactly one marker, on the diagnostic stream so it never
                // pollutes piped output.
                if !self.discarded {
                    self.discarded = true;
                    writeln!(self.err, "{}", self.style.wrap(Style::DIM, "[retrying]"))?;
                }
            }
            EventKind::Notice { level, message } => {
                let code = match level {
                    NoticeLevel::Error => Style::RED,
                    NoticeLevel::Warn => Style::YELLOW,
                    NoticeLevel::Info => Style::DIM,
                };
                writeln!(self.err, "{}", self.style.wrap(code, message))?;
            }
            EventKind::TurnEnded { outcome, .. } => {
                if self.wrote_any {
                    writeln!(self.out)?;
                }
                match outcome {
                    TurnOutcome::Completed => {}
                    TurnOutcome::Interrupted => {
                        writeln!(self.err, "{}", self.style.wrap(Style::DIM, "[interrupted]"))?
                    }
                    TurnOutcome::Refused { category, .. } => {
                        let detail = category
                            .as_deref()
                            .map(|c| format!(" ({c})"))
                            .unwrap_or_default();
                        writeln!(
                            self.err,
                            "{}",
                            self.style
                                .wrap(Style::YELLOW, &format!("[declined{detail}]"))
                        )?
                    }
                    TurnOutcome::StepLimit { steps } => writeln!(
                        self.err,
                        "{}",
                        self.style
                            .wrap(Style::YELLOW, &format!("[stopped after {steps} steps]"))
                    )?,
                    TurnOutcome::BudgetExceeded {
                        spent_usd,
                        limit_usd,
                    } => writeln!(
                        self.err,
                        "{}",
                        self.style.wrap(
                            Style::YELLOW,
                            &format!("[budget exceeded: ${spent_usd:.2} of ${limit_usd:.2}]")
                        )
                    )?,
                    TurnOutcome::Failed { message } => writeln!(
                        self.err,
                        "{}",
                        self.style.wrap(Style::RED, &format!("error: {message}"))
                    )?,
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.out.flush()?;
        self.err.flush()
    }
}

/// One JSON object per line, the event verbatim.
pub struct JsonlRenderer<W: Write> {
    out: W,
}

impl<W: Write> JsonlRenderer<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write> Renderer for JsonlRenderer<W> {
    fn handle(&mut self, event: &Event) -> std::io::Result<()> {
        // A protocol type that fails to serialise would silently truncate the
        // stream, so this is a hard error rather than a skipped line.
        let line = serde_json::to_string(event).map_err(std::io::Error::other)?;
        writeln!(self.out, "{line}")?;
        self.out.flush()
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// The prompt a one-shot invocation actually sends.
///
/// Piped stdin is appended rather than discarded: `cat f | axio -p "review
/// this"` is the single most common invocation, and dropping either half of it
/// silently produces a confident answer to the wrong question.
pub fn compose_prompt(flag: Option<&str>, stdin: Option<&str>) -> Option<String> {
    let flag = flag.map(str::trim).filter(|s| !s.is_empty());
    let stdin = stdin.map(str::trim).filter(|s| !s.is_empty());
    match (flag, stdin) {
        (Some(p), Some(s)) => Some(format!("{p}\n\n<stdin>\n{s}\n</stdin>")),
        (Some(p), None) => Some(p.to_owned()),
        (None, Some(s)) => Some(s.to_owned()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_core::protocol::{ItemId, SessionId, Usage};

    fn event(kind: EventKind) -> Event {
        Event {
            seq: 1,
            session: SessionId::nil(),
            turn: None,
            at_ms: 0,
            kind,
        }
    }

    fn text_delta(text: &str) -> Event {
        event(EventKind::ItemDelta {
            id: ItemId::nil(),
            delta: Delta::Text { text: text.into() },
        })
    }

    fn ended(outcome: TurnOutcome) -> Event {
        event(EventKind::TurnEnded {
            outcome,
            usage: Usage::default(),
            files_changed: vec![],
        })
    }

    fn render(events: &[Event], style: Style) -> (String, String) {
        let mut out = Vec::new();
        let err = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        {
            let mut r = PlainRenderer::new(&mut out, Box::new(Shared(err.clone())), style);
            for e in events {
                r.handle(e).unwrap();
            }
            r.finish().unwrap();
        }
        let e = String::from_utf8(err.lock().unwrap().clone()).unwrap();
        (String::from_utf8(out).unwrap(), e)
    }

    #[test]
    fn plain_output_contains_no_escape_bytes_when_not_a_terminal() {
        let (out, err) = render(
            &[
                text_delta("hello"),
                event(EventKind::Notice {
                    level: NoticeLevel::Warn,
                    message: "slow".into(),
                }),
                ended(TurnOutcome::Completed),
            ],
            Style::PLAIN,
        );
        assert!(!out.contains('\x1b'), "stdout carried an escape sequence");
        assert!(!err.contains('\x1b'), "stderr carried an escape sequence");
        assert_eq!(out, "hello\n");
    }

    #[test]
    fn colour_is_used_when_the_sink_is_a_terminal() {
        let (_out, err) = render(
            &[event(EventKind::Notice {
                level: NoticeLevel::Warn,
                message: "slow".into(),
            })],
            Style { colour: true },
        );
        assert!(err.contains('\x1b'));
    }

    #[test]
    fn the_answer_goes_to_stdout_and_diagnostics_to_stderr() {
        // Otherwise `axio -p x > out.txt` captures retry chatter as if it were
        // part of the answer.
        let (out, err) = render(
            &[
                text_delta("answer"),
                event(EventKind::ItemDiscarded {
                    id: ItemId::nil(),
                    reason: "overloaded".into(),
                }),
                ended(TurnOutcome::Completed),
            ],
            Style::PLAIN,
        );
        assert_eq!(out, "answer\n");
        assert!(err.contains("[retrying]"));
    }

    #[test]
    fn one_retry_marker_regardless_of_how_many_discards_arrive() {
        let discard = event(EventKind::ItemDiscarded {
            id: ItemId::nil(),
            reason: "overloaded".into(),
        });
        let (_out, err) = render(&[discard.clone(), discard.clone(), discard], Style::PLAIN);
        assert_eq!(err.matches("[retrying]").count(), 1);
    }

    #[test]
    fn a_failure_is_reported_on_stderr_and_not_mixed_into_the_answer() {
        let (out, err) = render(
            &[ended(TurnOutcome::Failed {
                message: "authentication failed".into(),
            })],
            Style::PLAIN,
        );
        assert_eq!(out, "");
        assert!(err.contains("error: authentication failed"));
    }

    #[test]
    fn jsonl_emits_one_object_per_line() {
        let mut buf = Vec::new();
        {
            let mut r = JsonlRenderer::new(&mut buf);
            r.handle(&text_delta("a")).unwrap();
            r.handle(&ended(TurnOutcome::Completed)).unwrap();
            r.finish().unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).expect("each line is one object");
            assert!(v.get("type").is_some());
            assert!(v.get("seq").is_some());
        }
    }

    #[test]
    fn stdin_is_appended_to_the_prompt_never_dropped() {
        assert_eq!(
            compose_prompt(Some("review this"), Some("fn main() {}")),
            Some("review this\n\n<stdin>\nfn main() {}\n</stdin>".to_owned())
        );
        assert_eq!(compose_prompt(Some("hi"), None), Some("hi".to_owned()));
        assert_eq!(compose_prompt(None, Some("hi")), Some("hi".to_owned()));
        assert_eq!(compose_prompt(None, None), None);
        // Whitespace-only stdin is not content.
        assert_eq!(
            compose_prompt(Some("hi"), Some("  \n ")),
            Some("hi".to_owned())
        );
    }
}
