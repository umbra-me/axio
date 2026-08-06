//! What the live area shows when it is not showing a streaming answer.
//!
//! The inline viewport cannot grow, so the slash menu and the credential form
//! borrow the rows the streaming tail uses rather than floating over the
//! transcript. Neither can be on screen during a turn, so the three never
//! compete for the space.
//!
//! Split from `paint` for the reason the repository splits anything: that file
//! reached the width limit, and a fifth painter would have taken it past.

use super::*;

impl Tui {
    /// The slash menu: names in a column, what each does beside it.
    ///
    /// Two columns rather than one line per entry with the description in
    /// brackets — a ragged right edge is what makes a list of names scannable,
    /// and scanning is the entire job of a menu that opens on one keystroke.
    pub(super) fn menu_rows(&self, area: Rect) -> Paragraph<'static> {
        let text = self.composer.text();
        let rows = area.height as usize;
        let (first, shown) = self.menu.window(text, rows);
        if shown.is_empty() {
            return Paragraph::new(Line::styled(
                "  no command matches",
                Style::default().add_modifier(Modifier::DIM),
            ));
        }

        let selected = self.menu.index(text);
        // Measured across everything the filter allows, not just the visible
        // slice, so the description column does not jump as the list scrolls.
        let name_width = commands::matching(text)
            .iter()
            .map(|spec| spec.name.chars().count())
            .max()
            .unwrap_or(0);

        let lines = shown
            .into_iter()
            .enumerate()
            .map(|(i, spec)| {
                let here = first + i == selected;
                let (marker, name_style) = if here {
                    (
                        "› ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("  ", Style::default())
                };
                Line::from(vec![
                    Span::styled(marker, Style::default().fg(Color::Cyan)),
                    Span::styled(format!("{:<name_width$}", spec.name), name_style),
                    Span::styled(
                        format!("   {}", spec.blurb),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        Paragraph::new(lines)
    }

    /// The model list, numbered as the digit keys expect.
    ///
    /// The one in use carries a tick. Without it the list answers "what could I
    /// pick" and not "what am I on", and those are the same question at the
    /// moment someone opens it.
    pub(super) fn picker_rows(&self, picker: &Picker, area: Rect) -> Paragraph<'static> {
        if picker.is_empty() {
            return Paragraph::new(Line::styled(
                "  the provider listed no models",
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        let rows = area.height as usize;
        let (first, shown) = picker.window(rows);

        let selected = picker.index();
        let lines = shown
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let at = first + i;
                let here = at == selected;
                let running = *name == picker.current();
                Line::from(vec![
                    Span::styled(
                        if here { "› " } else { "  " },
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{:>2}. ", at + 1),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        name.clone(),
                        if here {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(
                        if running { "  ✓" } else { "" },
                        Style::default().fg(Color::Green),
                    ),
                    // Only on the provider stage, and only when it is the
                    // reason a row cannot be chosen.
                    Span::styled(
                        match picker.offers().and_then(|o| o.get(at)) {
                            Some(offer) if !offer.ready => "  not configured",
                            _ => "",
                        },
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        Paragraph::new(lines)
    }

    /// The credential form.
    ///
    /// What is typed is never drawn — only how much of it there is. The
    /// terminal's scrollback outlives the process, and a credential echoed
    /// into it is a credential on disk in a file nobody is guarding.
    pub(super) fn login_rows(&self, login: &Login, area: Rect) -> Paragraph<'static> {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let rows = area.height as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();

        match login.stage() {
            LoginStage::Provider => {
                let chosen = login.provider_index();
                let providers = axio_core::auth::PROVIDERS;

                // Windowed around the selection, like the menu. It used to draw
                // the whole list on the assumption that three providers fit in
                // about three rows, and left a note saying a fourth would need
                // this. A fourth arrived. The tail below drains from the *front*
                // when it overflows, so the effect was the question and the
                // first providers scrolling silently off the top — the
                // highlight could be on a row that was no longer drawn.
                //
                // The question earns a row only when the list does not need it.
                let header = rows > providers.len();
                if header {
                    lines.push(Line::styled("  credential for which provider?", dim));
                }
                let room = rows.saturating_sub(usize::from(header)).max(1);
                let first = chosen
                    .saturating_sub(room.saturating_sub(1))
                    .min(providers.len().saturating_sub(room));

                for (i, provider) in providers.iter().enumerate().skip(first).take(room) {
                    let here = i == chosen;
                    lines.push(Line::from(vec![
                        Span::styled(
                            if here { "› " } else { "  " },
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::styled(
                            (*provider).to_owned(),
                            if here {
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            },
                        ),
                    ]));
                }
            }
            LoginStage::Secret => {
                lines.push(Line::styled(
                    format!("  credential for `{}`", login.provider()),
                    dim,
                ));
                // A fixed number of bullets would hide whether anything is
                // arriving at all; the true length is the one safe fact.
                let shown = login.typed_len().min(area.width.saturating_sub(6) as usize);
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled("•".repeat(shown), Style::default().fg(Color::Cyan)),
                ]));
                lines.push(Line::styled(
                    format!("  {} characters · enter to store", login.typed_len()),
                    dim,
                ));
            }
        }

        if lines.len() > rows {
            lines.drain(..lines.len() - rows);
        }
        Paragraph::new(lines)
    }
}
