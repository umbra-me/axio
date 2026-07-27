//! Measuring and breaking text, in columns rather than characters.
//!
//! One implementation of each rule: word wrapping for prose, width-only
//! breaking for code, and the truncation that marks what it cut. A second copy
//! of any of them drifts, and the first anybody notices is two paragraphs
//! breaking differently on one screen.

use super::*;

/// Columns a character occupies on screen, which is not how many characters it
/// is. A CJK ideograph and most emoji take two; a combining mark takes none.
/// Counting characters instead means every such line is written a column too
/// wide and loses its right-hand end to the clip.
///
/// Measured through ratatui's own text measurement rather than a second
/// dependency, so the renderer and the terminal cannot disagree about it.
pub fn cell_width(c: char) -> usize {
    let mut buffer = [0u8; 4];
    Span::raw(&*c.encode_utf8(&mut buffer)).width()
}

/// Columns a string occupies on screen.
pub fn text_width(text: &str) -> usize {
    Span::raw(text).width()
}

/// Word-wrap unstyled text to a width in columns.
///
/// The same wrapping the renderer uses, for callers with nothing to style — a
/// typed prompt going to scrollback, a command preview, the streaming tail. A
/// second implementation would drift from this one, and the first anybody would
/// hear of it is two paragraphs breaking differently on the same screen.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(MIN_WIDTH);
    text.split('\n')
        .flat_map(|paragraph| {
            let chars: Vec<(char, Style)> =
                paragraph.chars().map(|c| (c, Style::default())).collect();
            wrap_styled(&chars, width)
                .into_iter()
                .map(|row| row.into_iter().map(|(c, _)| c).collect::<String>())
        })
        .collect()
}

/// Cut unstyled text to a width in columns, marking the cut.
pub fn truncate(text: &str, width: usize) -> String {
    fit(vec![(text.to_owned(), Style::default())], width)
        .into_iter()
        .map(|(t, _)| t)
        .collect()
}

/// Lay styled runs out under a prefix, wrapping on word boundaries. `first`
/// leads the opening row and `hang` every row after it, so a bullet's text
/// stays in one column.
pub(super) fn compose(
    first: Span<'static>,
    hang: Span<'static>,
    segs: Vec<(String, Style)>,
    width: usize,
) -> Vec<Line<'static>> {
    let prefix = first.width();
    let avail = width.saturating_sub(prefix).max(MIN_WIDTH);
    let styled: Vec<(char, Style)> = segs
        .iter()
        .flat_map(|(text, style)| text.chars().map(move |c| (c, *style)))
        .collect();

    let rows = wrap_styled(&styled, avail);
    if rows.is_empty() {
        return vec![Line::from(vec![first])];
    }
    rows.into_iter()
        .enumerate()
        .map(|(i, row)| {
            let lead = if i == 0 { first.clone() } else { hang.clone() };
            let mut spans = vec![lead];
            spans.extend(merge(&row));
            Line::from(spans)
        })
        .collect()
}

/// Greedy word wrapping over styled characters. A word wider than the line is
/// broken rather than dropped.
pub(super) fn wrap_styled(chars: &[(char, Style)], width: usize) -> Vec<Vec<(char, Style)>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut row: Vec<(char, Style)> = Vec::new();
    let mut used = 0usize;
    let mut last_space: Option<usize> = None;

    for &(c, style) in chars {
        let cell = cell_width(c);
        if used + cell > width && !row.is_empty() {
            match last_space {
                Some(at) => {
                    let rest = row.split_off(at + 1);
                    row.pop();
                    rows.push(std::mem::replace(&mut row, rest));
                }
                None => rows.push(std::mem::take(&mut row)),
            }
            used = row.iter().map(|(c, _)| cell_width(*c)).sum();
            last_space = row.iter().rposition(|(c, _)| *c == ' ');
        }
        // A row never opens with the space it was broken on.
        if c == ' ' && row.is_empty() {
            continue;
        }
        if c == ' ' {
            last_space = Some(row.len());
        }
        row.push((c, style));
        used += cell;
    }
    rows.push(row);
    rows
}

/// Break styled runs on width alone, never on a space. Used for code, where a
/// space is indentation and moving it moves the meaning.
pub(super) fn hard_wrap_styled(chars: &[(char, Style)], width: usize) -> Vec<Vec<(char, Style)>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<(char, Style)>> = vec![Vec::new()];
    let mut used = 0usize;
    for &(c, style) in chars {
        let cell = cell_width(c);
        if used + cell > width && !rows.last().expect("a row").is_empty() {
            rows.push(Vec::new());
            used = 0;
        }
        rows.last_mut().expect("a row").push((c, style));
        used += cell;
    }
    rows
}

/// Consecutive characters sharing a style become one span, so a wrapped
/// paragraph costs a handful of spans rather than one per character.
pub(super) fn merge(row: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut text = String::new();
    let mut current: Option<Style> = None;

    for &(c, style) in row {
        if current != Some(style) {
            if let Some(previous) = current {
                spans.push(Span::styled(std::mem::take(&mut text), previous));
            }
            current = Some(style);
        }
        text.push(c);
    }
    if let Some(style) = current {
        spans.push(Span::styled(text, style));
    }
    spans
}

/// Break on width alone, for text where a space carries no more meaning than
/// any other character.
pub(super) fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut rows = vec![String::new()];
    let mut used = 0usize;
    for c in text.chars() {
        let cell = cell_width(c);
        if used + cell > width && !rows.last().expect("a row").is_empty() {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().expect("a row").push(c);
        used += cell;
    }
    rows
}
