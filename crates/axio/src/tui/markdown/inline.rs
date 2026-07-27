//! Markup inside a line: emphasis, code, links.
//!
//! Unrecognised or unclosed markers stay literal, because the renderer is the
//! last thing to see the text and a marker it cannot resolve is still what the
//! model wrote.

use super::*;

/// Split text into styled runs. Unrecognised or unclosed markers stay literal.
pub(super) fn inline(src: &str, depth: usize) -> Vec<(String, Style)> {
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
pub(super) fn emphasis(
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
pub(super) fn link(
    chars: &[char],
    at: usize,
    depth: usize,
) -> Option<(Vec<(String, Style)>, usize)> {
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
pub(super) fn closing(chars: &[char], from: usize, marker: &[char]) -> Option<usize> {
    if from >= chars.len() || marker.is_empty() {
        return None;
    }
    (from..=chars.len().saturating_sub(marker.len()))
        .find(|&i| chars[i..i + marker.len()] == *marker)
}
