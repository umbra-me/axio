//! Tables, which are the one construct a line cannot settle.
//!
//! Columns are as wide as their widest cell, so nothing can be drawn until the
//! row that ends the table has arrived.

use super::*;

/// How a column's cells sit in the space they are given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Left,
    Right,
    Centre,
}

/// The cells of a table row, or nothing if the line is not one.
///
/// A leading pipe is what identifies a row: a sentence with a pipe in the
/// middle of it is a sentence, and treating it as a table would eat it.
pub(super) fn table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|')?;
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    Some(inner.split('|').map(|c| c.trim().to_owned()).collect())
}

/// `---`, `:--`, `--:` or `:-:` in every cell: the row that says where the
/// header ends and how the columns align, rather than a row of data.
pub(super) fn alignments(cells: &[String]) -> Option<Vec<Align>> {
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
pub(super) fn table(rows: &[Vec<String>], width: usize) -> Vec<Line<'static>> {
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

pub(super) fn segs_width(segs: &[(String, Style)]) -> usize {
    segs.iter().map(|(t, _)| text_width(t)).sum()
}

/// Cut styled runs down to `width` columns, marking the cut.
pub(super) fn fit(segs: Vec<(String, Style)>, width: usize) -> Vec<(String, Style)> {
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
pub(super) fn pad(segs: Vec<(String, Style)>, width: usize, align: Align) -> Vec<Span<'static>> {
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
