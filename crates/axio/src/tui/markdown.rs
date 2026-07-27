//! Markdown as terminal lines.
//!
//! The model writes structure as markdown whether or not anything renders it,
//! so leaving the text raw does not mean showing what the model meant — it
//! means showing asterisks. This turns the subset the model actually emits into
//! styles, and leaves anything it does not understand as the characters that
//! were written, which is the only safe failure mode for a renderer that never
//! sees the source again.
//!
//! **A line at a time**, because the surface renders a message while it is
//! still streaming: the only state that crosses a line boundary is an open code
//! fence, and that is what [`Renderer`] holds. Rendering a whole message in one
//! call and rendering it line by line produce identical output — there is a
//! test for exactly that, because the streaming path relies on it.
//!
//! Every returned line is already wrapped to the width it was given and carries
//! its own two-column gutter, so a caller can hand the result straight to
//! `insert_before` with `lines.len()` as the height and never have one clipped.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// The left margin every rendered line carries, matching the rest of the
/// transcript.
const GUTTER: &str = "  ";

/// Narrower than this and wrapping stops being wrapping. A terminal this small
/// is already broken; the renderer just refuses to make it worse by emitting
/// one character per line.
const MIN_WIDTH: usize = 20;

/// How deep emphasis may nest before the parser stops looking. Bold inside a
/// list item inside a quote is real; four levels is a pathological string of
/// asterisks, and recursion is not the way to find out.
const MAX_NESTING: usize = 4;

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

fn code() -> Style {
    Style::default().fg(Color::Cyan)
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Renders markdown into styled lines, carrying the only state that outlives a
/// single line.
#[derive(Default)]
pub struct Renderer {
    /// Inside a fenced code block, where nothing is markdown any more.
    fenced: bool,
    /// The parser for the fence being read, if its language is one we know.
    /// It carries state between lines, because a string or a block comment
    /// opened on one line is still open on the next.
    code: Option<super::highlight::Highlighter>,
    /// Rows of a table being collected. A table is the one construct that
    /// cannot be rendered a line at a time: its columns are as wide as their
    /// widest cell, and the widest cell may not have arrived yet.
    table: Vec<Vec<String>>,
}

impl Renderer {
    /// Render a whole block of text. Equivalent to calling [`Renderer::line`]
    /// on each of its lines in order.
    pub fn block(&mut self, text: &str, width: usize) -> Vec<Line<'static>> {
        text.split('\n').flat_map(|l| self.line(l, width)).collect()
    }

    /// Anything held back waiting for more input — which is a table that ran to
    /// the end of the message. A caller that stops feeding lines without
    /// calling this loses it.
    pub fn finish(&mut self, width: usize) -> Vec<Line<'static>> {
        if self.table.is_empty() {
            return Vec::new();
        }
        let rows = std::mem::take(&mut self.table);
        table(&rows, width.saturating_sub(GUTTER.len()).max(MIN_WIDTH))
    }

    /// Render one source line, which may become several terminal rows once it
    /// is wrapped — or none at all, for a fence marker that only changes state.
    pub fn line(&mut self, src: &str, width: usize) -> Vec<Line<'static>> {
        let src = src.strip_suffix('\r').unwrap_or(src);
        let body = width.saturating_sub(GUTTER.len()).max(MIN_WIDTH);

        // A table row joins the one being collected; anything else ends it, and
        // the finished table is drawn before whatever ended it.
        if !self.fenced {
            if let Some(cells) = table_row(src) {
                self.table.push(cells);
                return Vec::new();
            }
            if !self.table.is_empty() {
                let mut lines = self.finish(width);
                lines.extend(self.line(src, width));
                return lines;
            }
        }

        if let Some(info) = fence(src) {
            // The marker itself is not content: the styling says "code", and a
            // row of backticks in the transcript says nothing. What the marker
            // does carry is the language, which is the only chance to know it.
            self.fenced = !self.fenced;
            self.code = if self.fenced {
                super::highlight::Highlighter::new(info)
            } else {
                None
            };
            return Vec::new();
        }
        if self.fenced {
            let Some(highlighter) = self.code.as_mut() else {
                return hard_wrap(src, body)
                    .into_iter()
                    .map(|row| Line::from(vec![Span::raw(GUTTER), Span::styled(row, code())]))
                    .collect();
            };
            // Wrapped on width alone, and never on a space: a space in code is
            // indentation, and moving it moves the meaning.
            let runs = highlighter.line(src);
            let chars: Vec<(char, Style)> = runs
                .iter()
                .flat_map(|(text, style)| text.chars().map(move |c| (c, *style)))
                .collect();
            return hard_wrap_styled(&chars, body)
                .into_iter()
                .map(|row| {
                    let mut spans = vec![Span::raw(GUTTER)];
                    spans.extend(merge(&row));
                    Line::from(spans)
                })
                .collect();
        }

        let trimmed = src.trim_start();
        if trimmed.is_empty() {
            return vec![Line::raw("")];
        }
        let indent = " ".repeat(src.chars().count() - trimmed.chars().count());

        if is_rule(trimmed) {
            return vec![Line::from(vec![
                Span::raw(GUTTER),
                Span::styled("─".repeat(body), dim()),
            ])];
        }

        if let Some((level, rest)) = heading(trimmed) {
            let style = if level <= 2 {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            let segs = inline(rest, 0)
                .into_iter()
                .map(|(t, s)| (t, style.patch(s)))
                .collect();
            return compose(Span::raw(GUTTER), Span::raw(GUTTER), segs, width);
        }

        if let Some(rest) = quote(trimmed) {
            let bar = format!("{GUTTER}{indent}│ ");
            let segs = inline(rest, 0)
                .into_iter()
                .map(|(t, s)| (t, dim().patch(s)))
                .collect();
            return compose(
                Span::styled(bar.clone(), dim()),
                Span::styled(bar, dim()),
                segs,
                width,
            );
        }

        if let Some((marker, rest)) = list_item(trimmed) {
            // A wrapped list item hangs under its own text rather than under
            // its bullet, so the marker column stays readable at a glance.
            let first = format!("{GUTTER}{indent}{marker} ");
            let hang = " ".repeat(first.chars().count());
            return compose(
                Span::styled(first, Style::default().fg(Color::Cyan)),
                Span::raw(hang),
                inline(rest, 0),
                width,
            );
        }

        let pad = format!("{GUTTER}{indent}");
        compose(
            Span::raw(pad.clone()),
            Span::raw(pad),
            inline(trimmed, 0),
            width,
        )
    }
}

/// Inline markdown for text that is not a whole line — the tail of a message
/// still streaming, where the closing marker may not have arrived yet. An
/// unclosed marker renders as itself, which is what it will be until it closes.
pub fn spans(text: &str, style: Style) -> Vec<Span<'static>> {
    inline(text, 0)
        .into_iter()
        .map(|(t, s)| Span::styled(t, style.patch(s)))
        .collect()
}

// ------------------------------------------------------------------- blocks

/// A fence marker, and the info string after it — the language, when the model
/// bothered to say. An empty string is still a fence; `None` is not one.
fn fence(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t.strip_prefix("```").or_else(|| t.strip_prefix("~~~"))?;
    Some(rest.trim())
}

fn is_rule(line: &str) -> bool {
    let t = line.trim_end();
    let Some(first) = t.chars().next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_') && t.chars().count() >= 3 && t.chars().all(|c| c == first)
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?;
    Some((hashes, rest.trim_start()))
}

fn quote(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// A bullet or an ordered item, as the marker to print and the text after it.
/// A bullet becomes `•`; an ordered marker keeps the number the model chose,
/// because renumbering it would contradict prose that refers to "step 3".
fn list_item(line: &str) -> Option<(String, &str)> {
    for bullet in ['-', '*', '+'] {
        if let Some(rest) = line
            .strip_prefix(bullet)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            return Some(("•".to_owned(), rest.trim_start()));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let after = &line[digits..];
    let delim = after.chars().next().filter(|c| matches!(c, '.' | ')'))?;
    let rest = after[1..].strip_prefix(' ')?;
    Some((format!("{}{delim}", &line[..digits]), rest.trim_start()))
}

// -------------------------------------------------------------------- tables

/// How a column's cells sit in the space they are given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Right,
    Centre,
}

/// The cells of a table row, or nothing if the line is not one.
///
/// A leading pipe is what identifies a row: a sentence with a pipe in the
/// middle of it is a sentence, and treating it as a table would eat it.
fn table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|')?;
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    Some(inner.split('|').map(|c| c.trim().to_owned()).collect())
}

/// `---`, `:--`, `--:` or `:-:` in every cell: the row that says where the
/// header ends and how the columns align, rather than a row of data.
fn alignments(cells: &[String]) -> Option<Vec<Align>> {
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|cell| {
            let body = cell.trim();
            let stripped = body.trim_start_matches(':').trim_end_matches(':');
            if stripped.is_empty() || !stripped.chars().all(|c| c == '-') {
                return None;
            }
            Some(match (body.starts_with(':'), body.ends_with(':')) {
                (true, true) => Align::Centre,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

/// Draw a collected table, fitted to the width it has.
///
/// Columns are as wide as their widest cell until they do not fit, at which
/// point the widest column gives up a column at a time — narrowing what has the
/// most to give rather than truncating every cell equally.
fn table(rows: &[Vec<String>], width: usize) -> Vec<Line<'static>> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return Vec::new();
    }

    let divider = rows.iter().position(|r| alignments(r).is_some());
    let align: Vec<Align> = divider
        .and_then(|at| alignments(&rows[at]))
        .unwrap_or_default();
    let align = |column: usize| align.get(column).copied().unwrap_or(Align::Left);

    let cell = |row: &Vec<String>, column: usize| -> Vec<(String, Style)> {
        row.get(column).map(|c| inline(c, 0)).unwrap_or_default()
    };
    let body: Vec<&Vec<String>> = rows
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != divider)
        .map(|(_, r)| r)
        .collect();

    // Natural widths, then shaved down until the whole thing fits.
    let mut widths: Vec<usize> = (0..columns)
        .map(|c| {
            body.iter()
                .map(|row| segs_width(&cell(row, c)))
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    let separators = 3 * columns.saturating_sub(1);
    while widths.iter().sum::<usize>() + separators > width {
        let Some(widest) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .map(|(i, _)| i)
        else {
            break;
        };
        if widths[widest] <= 1 {
            break;
        }
        widths[widest] -= 1;
    }

    let dim_bar = Span::styled(" │ ", dim());
    let mut lines = Vec::new();
    for (n, row) in body.iter().enumerate() {
        // The row above the divider is a header, and reads as one.
        let header = divider.is_some_and(|at| n < at);
        let mut spans = vec![Span::raw(GUTTER)];
        for (column, room) in widths.iter().enumerate() {
            if column > 0 {
                spans.push(dim_bar.clone());
            }
            let mut segs = fit(cell(row, column), *room);
            if header {
                segs = segs
                    .into_iter()
                    .map(|(t, s)| (t, s.patch(Style::default().add_modifier(Modifier::BOLD))))
                    .collect();
            }
            spans.extend(pad(segs, *room, align(column)));
        }
        lines.push(Line::from(spans));

        if header && divider.is_some_and(|at| n + 1 == at) {
            let rule: String = widths
                .iter()
                .map(|w| "─".repeat(*w))
                .collect::<Vec<_>>()
                .join("─┼─");
            lines.push(Line::from(vec![
                Span::raw(GUTTER),
                Span::styled(rule, dim()),
            ]));
        }
    }
    lines
}

fn segs_width(segs: &[(String, Style)]) -> usize {
    segs.iter().map(|(t, _)| text_width(t)).sum()
}

/// Cut styled runs down to `width` columns, marking the cut.
fn fit(segs: Vec<(String, Style)>, width: usize) -> Vec<(String, Style)> {
    if segs_width(&segs) <= width {
        return segs;
    }
    let room = width.saturating_sub(1);
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut used = 0usize;
    for (text, style) in segs {
        let mut kept = String::new();
        for c in text.chars() {
            let cell = cell_width(c);
            if used + cell > room {
                break;
            }
            kept.push(c);
            used += cell;
        }
        if !kept.is_empty() {
            out.push((kept, style));
        }
        if used >= room {
            break;
        }
    }
    out.push(("…".to_owned(), dim()));
    out
}

/// Place a cell's runs in a column of exactly `width`.
fn pad(segs: Vec<(String, Style)>, width: usize, align: Align) -> Vec<Span<'static>> {
    let slack = width.saturating_sub(segs_width(&segs));
    let (before, after) = match align {
        Align::Left => (0, slack),
        Align::Right => (slack, 0),
        Align::Centre => (slack / 2, slack - slack / 2),
    };
    let mut spans = Vec::new();
    if before > 0 {
        spans.push(Span::raw(" ".repeat(before)));
    }
    spans.extend(segs.into_iter().map(|(t, s)| Span::styled(t, s)));
    if after > 0 {
        spans.push(Span::raw(" ".repeat(after)));
    }
    spans
}

// ------------------------------------------------------------------- inline

/// Split text into styled runs. Unrecognised or unclosed markers stay literal.
fn inline(src: &str, depth: usize) -> Vec<(String, Style)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let doubled = chars.get(i + 1) == Some(&c);

        let taken = match c {
            // Code first and unconditionally: inside backticks nothing else is
            // markup, which is the whole point of writing it in backticks.
            '`' => closing(&chars, i + 1, &['`']).map(|end| {
                (
                    vec![(chars[i + 1..end].iter().collect::<String>(), code())],
                    end + 1,
                )
            }),
            '*' | '_' if doubled => emphasis(
                &chars,
                i,
                2,
                Style::default().add_modifier(Modifier::BOLD),
                depth,
            ),
            '~' if doubled => emphasis(
                &chars,
                i,
                2,
                Style::default().add_modifier(Modifier::CROSSED_OUT),
                depth,
            ),
            // `snake_case_name` is an identifier, not two italics. A single
            // underscore only opens emphasis at a word boundary; an asterisk
            // has no such ambiguity and needs no such guard.
            '_' if i > 0 && chars[i - 1].is_alphanumeric() => None,
            '*' | '_' => emphasis(
                &chars,
                i,
                1,
                Style::default().add_modifier(Modifier::ITALIC),
                depth,
            ),
            '[' => link(&chars, i, depth),
            _ => None,
        };

        match taken {
            Some((runs, next)) => {
                if !plain.is_empty() {
                    out.push((std::mem::take(&mut plain), Style::default()));
                }
                out.extend(runs);
                i = next;
            }
            None => {
                plain.push(c);
                i += 1;
            }
        }
    }
    if !plain.is_empty() {
        out.push((plain, Style::default()));
    }
    out
}

/// `**bold**`, `*italic*`, `~~struck~~` — the marker repeated `len` times on
/// both sides. The content is parsed again so emphasis can hold code, with the
/// outer style patched underneath whatever the inner run asked for.
fn emphasis(
    chars: &[char],
    at: usize,
    len: usize,
    style: Style,
    depth: usize,
) -> Option<(Vec<(String, Style)>, usize)> {
    if depth >= MAX_NESTING {
        return None;
    }
    let marker = vec![chars[at]; len];
    let start = at + len;
    // Emphasis is glued to the text it emphasises: `2 * 3` is arithmetic, and
    // `a * b * c` is not one italic word. A marker followed by a space opens
    // nothing, and one preceded by a space closes nothing.
    if chars.get(start).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let mut from = start;
    let end = loop {
        let candidate = closing(chars, from, &marker)?;
        if candidate > start && !chars[candidate - 1].is_whitespace() {
            break candidate;
        }
        from = candidate + 1;
    };
    let inner: String = chars[start..end].iter().collect();
    let runs = inline(&inner, depth + 1)
        .into_iter()
        .map(|(t, s)| (t, style.patch(s)))
        .collect();
    Some((runs, end + len))
}

/// `[text](url)`. The URL is kept — a terminal has nowhere to hide it, and a
/// link whose destination is invisible is worse than one that is merely long.
fn link(chars: &[char], at: usize, depth: usize) -> Option<(Vec<(String, Style)>, usize)> {
    if depth >= MAX_NESTING {
        return None;
    }
    let text_end = closing(chars, at + 1, &[']'])?;
    if chars.get(text_end + 1) != Some(&'(') {
        return None;
    }
    let url_end = closing(chars, text_end + 2, &[')'])?;
    let text: String = chars[at + 1..text_end].iter().collect();
    let url: String = chars[text_end + 2..url_end].iter().collect();

    let label = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED);
    let mut runs: Vec<(String, Style)> = inline(&text, depth + 1)
        .into_iter()
        .map(|(t, s)| (t, label.patch(s)))
        .collect();
    runs.push((format!(" ({url})"), dim()));
    Some((runs, url_end + 1))
}

/// The index at which `marker` next occurs at or after `from`.
fn closing(chars: &[char], from: usize, marker: &[char]) -> Option<usize> {
    if from >= chars.len() || marker.is_empty() {
        return None;
    }
    (from..=chars.len().saturating_sub(marker.len()))
        .find(|&i| chars[i..i + marker.len()] == *marker)
}

// ------------------------------------------------------------------ wrapping

/// Lay styled runs out under a prefix, wrapping on word boundaries. `first`
/// leads the opening row and `hang` every row after it, so a bullet's text
/// stays in one column.
fn compose(
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
fn wrap_styled(chars: &[(char, Style)], width: usize) -> Vec<Vec<(char, Style)>> {
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

/// Consecutive characters sharing a style become one span, so a wrapped
/// paragraph costs a handful of spans rather than one per character.
fn merge(row: &[(char, Style)]) -> Vec<Span<'static>> {
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

/// Break styled runs on width alone, never on a space. Used for code, where a
/// space is indentation and moving it moves the meaning.
fn hard_wrap_styled(chars: &[(char, Style)], width: usize) -> Vec<Vec<(char, Style)>> {
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

/// Break on width alone, for text where a space carries no more meaning than
/// any other character.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn styles_of(lines: &[Line<'static>]) -> Vec<(String, Style)> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| (s.content.to_string(), s.style)))
            .collect()
    }

    fn render(src: &str, width: usize) -> Vec<Line<'static>> {
        Renderer::default().block(src, width)
    }

    #[test]
    fn bold_loses_its_asterisks_and_gains_the_modifier() {
        let lines = render("a **loud** word", 40);
        assert_eq!(text_of(&lines), "  a loud word");
        let bold = styles_of(&lines)
            .into_iter()
            .find(|(t, _)| t == "loud")
            .expect("the bold run");
        assert!(bold.1.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_single_asterisk_pair_is_italic() {
        let lines = render("*Introduction*", 40);
        assert_eq!(text_of(&lines), "  Introduction");
        assert!(
            styles_of(&lines)
                .iter()
                .any(|(t, s)| t == "Introduction" && s.add_modifier.contains(Modifier::ITALIC))
        );
    }

    #[test]
    fn inline_code_keeps_its_text_and_drops_its_backticks() {
        let lines = render("run `cargo test` first", 40);
        assert_eq!(text_of(&lines), "  run cargo test first");
        assert!(
            styles_of(&lines)
                .iter()
                .any(|(t, s)| t == "cargo test" && s.fg == Some(Color::Cyan))
        );
    }

    #[test]
    fn an_identifier_is_not_two_italics() {
        // The failure this covers: `snake_case_name` rendered as "snakecasename"
        // with the middle in italics, silently changing an identifier.
        let lines = render("call snake_case_name now", 60);
        assert_eq!(text_of(&lines), "  call snake_case_name now");
    }

    #[test]
    fn an_unclosed_marker_stays_literal() {
        let lines = render("2 * 3 and **half open", 60);
        assert_eq!(text_of(&lines), "  2 * 3 and **half open");
    }

    #[test]
    fn arithmetic_is_not_emphasis() {
        // The failure this covers: `2 * 3 * 4` rendering as "2 3 4" with the
        // middle in italics, because a lone asterisk found a partner further
        // down the line.
        let lines = render("2 * 3 * 4 and a_b * c", 60);
        assert_eq!(text_of(&lines), "  2 * 3 * 4 and a_b * c");
    }

    #[test]
    fn a_heading_loses_its_hashes_and_is_bold() {
        let lines = render("## Body", 40);
        assert_eq!(text_of(&lines), "  Body");
        assert!(
            styles_of(&lines)
                .iter()
                .any(|(t, s)| t == "Body" && s.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn a_bullet_becomes_a_marker_and_wraps_under_its_own_text() {
        let lines = render("- alpha beta gamma delta epsilon", 24);
        let rendered = text_of(&lines);
        assert!(rendered.starts_with("  • alpha"), "{rendered}");
        // Every continuation row lines up under the text, not under the bullet.
        for row in rendered.lines().skip(1) {
            assert!(row.starts_with("    "), "{row:?}");
        }
    }

    #[test]
    fn an_ordered_marker_keeps_the_number_the_model_chose() {
        // Prose that says "step 3" must still point at the item labelled 3.
        let lines = render("3. **Critical Thinking**", 40);
        assert_eq!(text_of(&lines), "  3. Critical Thinking");
    }

    #[test]
    fn a_fenced_block_renders_verbatim_without_its_fences() {
        let lines = render("before\n```rust\nlet **x** = 1;\n```\nafter", 40);
        assert_eq!(text_of(&lines), "  before\n  let **x** = 1;\n  after");
    }

    #[test]
    fn a_link_keeps_both_its_text_and_its_destination() {
        let lines = render("see [the docs](https://example.com)", 60);
        let rendered = text_of(&lines);
        assert!(rendered.contains("the docs"), "{rendered}");
        assert!(rendered.contains("https://example.com"), "{rendered}");
    }

    #[test]
    fn nothing_ever_exceeds_the_width_it_was_given() {
        let src = "# A heading that runs on\n\n- a bullet with a great many words in it indeed\n\n\
                   a paragraph containing an_extremely_long_unbroken_identifier_that_cannot_fit\n\
                   ```\nand a very long line of code that also cannot fit inside the width\n```";
        for width in [24usize, 40, 80] {
            for line in render(src, width) {
                let n: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                assert!(n <= width, "width {width}: {n} in {:?}", line);
            }
        }
    }

    #[test]
    fn a_wide_character_is_measured_in_columns_not_characters() {
        // The failure this covers: an ideograph counts as one character and
        // occupies two columns, so a line counted by character is written a
        // column too wide for every one of them and loses its end to the clip.
        let src = "日本語のテキストがここにあります and some latin words too";
        for width in [24usize, 30, 48] {
            for line in render(src, width) {
                let drawn: usize = line.spans.iter().map(|s| s.width()).sum();
                assert!(drawn <= width, "width {width}: drew {drawn}");
            }
        }
    }

    #[test]
    fn an_emoji_is_two_columns_wide() {
        assert_eq!(cell_width('✅'), 2);
        assert_eq!(cell_width('a'), 1);
        for line in render("✅ ✅ ✅ ✅ ✅ ✅ ✅ ✅ ✅ ✅ ✅ ✅", 24) {
            let drawn: usize = line.spans.iter().map(|s| s.width()).sum();
            assert!(drawn <= 24, "drew {drawn}");
        }
    }

    #[test]
    fn a_blank_line_survives() {
        assert_eq!(text_of(&render("a\n\nb", 40)), "  a\n\n  b");
    }

    #[test]
    fn streaming_a_message_line_by_line_renders_it_identically() {
        // The streaming path flushes each source line as its newline arrives.
        // If that diverged from rendering the whole message, a resumed or
        // re-rendered transcript would not match what was watched live.
        let src = "# Title\n\nsome **text** here\n\n```\ncode *stays* raw\n```\n\n- one\n- two\n";
        let whole = render(src, 36);

        let mut streamed = Renderer::default();
        let mut piecemeal = Vec::new();
        for line in src.split('\n') {
            piecemeal.extend(streamed.line(line, 36));
        }

        assert_eq!(text_of(&whole), text_of(&piecemeal));
        assert_eq!(styles_of(&whole), styles_of(&piecemeal));
    }

    /// Tables are held back until they end, so a test that only calls `block`
    /// sees nothing — the same trap the surface has to handle.
    fn render_all(src: &str, width: usize) -> Vec<Line<'static>> {
        let mut renderer = Renderer::default();
        let mut lines = renderer.block(src, width);
        lines.extend(renderer.finish(width));
        lines
    }

    const TABLE: &str = "| Fruit | Cost |\n|---|---:|\n| apple | 3 |\n| **plum** | 12 |";

    #[test]
    fn a_table_is_drawn_as_columns_rather_than_pipes() {
        let text = text_of(&render_all(TABLE, 40));
        assert!(!text.contains('|'), "{text}");
        assert!(text.contains("Fruit"), "{text}");
        // Aligned: every row's separator sits in the same column.
        let bars: Vec<usize> = text.lines().filter_map(|l| l.find('│')).collect();
        assert!(bars.windows(2).all(|w| w[0] == w[1]), "{text}");
        // The header is bold and the markers inside a cell are still rendered.
        assert!(
            styles_of(&render_all(TABLE, 40))
                .iter()
                .any(|(t, s)| t == "Fruit" && s.add_modifier.contains(Modifier::BOLD))
        );
        assert!(
            text.contains("plum") && !text.contains("**plum**"),
            "{text}"
        );
    }

    #[test]
    fn a_right_aligned_column_is_right_aligned() {
        let text = text_of(&render_all(TABLE, 40));
        let rows: Vec<&str> = text.lines().collect();
        let ends: Vec<&str> = rows
            .iter()
            .filter(|r| r.contains('3') || r.contains("12"))
            .copied()
            .collect();
        assert!(
            ends.iter().all(|r| r.ends_with('3') || r.ends_with("12")),
            "{text}"
        );
    }

    #[test]
    fn a_table_ends_where_the_prose_starts_again() {
        let src = format!("{TABLE}\n\nand then a sentence");
        let text = text_of(&render_all(&src, 40));
        assert!(text.contains("apple"), "{text}");
        assert!(text.contains("and then a sentence"), "{text}");
        assert_eq!(text.matches("apple").count(), 1, "{text}");
    }

    #[test]
    fn a_table_too_wide_for_the_terminal_is_narrowed_and_says_so() {
        let text = text_of(&render_all(
            "| Column | Another |\n|---|---|\n| a very long value indeed | short |",
            26,
        ));
        for line in text.lines() {
            assert!(text_width(line) <= 26, "{line:?} in {text}");
        }
        assert!(text.contains('…'), "{text}");
    }

    #[test]
    fn a_sentence_with_a_pipe_in_it_is_not_a_table() {
        let text = text_of(&render_all("run a | b to pipe it", 40));
        assert_eq!(text, "  run a | b to pipe it");
    }

    #[test]
    fn a_table_renders_the_same_whether_it_streamed_or_not() {
        let whole = render_all(TABLE, 36);
        let mut streamed = Renderer::default();
        let mut piecemeal = Vec::new();
        for line in TABLE.split('\n') {
            piecemeal.extend(streamed.line(line, 36));
        }
        piecemeal.extend(streamed.finish(36));
        assert_eq!(text_of(&whole), text_of(&piecemeal));
    }

    #[test]
    fn a_rule_is_drawn_rather_than_printed() {
        let lines = render("---", 30);
        assert_eq!(text_of(&lines), format!("  {}", "─".repeat(28)));
    }

    #[test]
    fn a_quote_is_marked_in_the_margin() {
        let lines = render("> quoted", 40);
        assert!(text_of(&lines).contains("│ quoted"));
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
}
