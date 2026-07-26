//! Colour for fenced code, in the terminal's own palette.
//!
//! Only syntect's **parser** is used, never its themes. A theme carries
//! absolute colours chosen against a particular background, and the background
//! here belongs to the user: a dark theme on a light terminal is unreadable,
//! and there is no way to ask which one they have. Scopes are mapped to the
//! sixteen colours the terminal already defines instead, so highlighted code
//! matches everything around it whatever the user's palette is.
//!
//! The whole thing degrades to nothing: an unknown language, an unparseable
//! line, or a fence with no language at all renders as plain code rather than
//! failing. A renderer is not worth an error.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

/// Built once and shared. Loading the syntax definitions is measurable, and a
/// transcript can hold a great many code blocks.
fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    // The no-newline set, because the renderer works in lines that have already
    // had their terminator removed.
    SYNTAXES.get_or_init(two_face::syntax::extra_no_newlines)
}

/// Scope prefixes, most specific first, and what the terminal should make of
/// them. The first that matches from the top of the scope stack downwards wins,
/// so `meta.function.rust` wrapping `keyword.other.fn.rust` colours the keyword.
const SCOPES: [(&str, Color); 12] = [
    ("comment", Color::DarkGray),
    ("string", Color::Green),
    ("constant.numeric", Color::Yellow),
    ("constant.language", Color::Yellow),
    ("constant.character", Color::Green),
    ("keyword", Color::Magenta),
    ("storage", Color::Magenta),
    ("entity.name.function", Color::Blue),
    ("entity.name", Color::Cyan),
    ("support.function", Color::Blue),
    ("support", Color::Cyan),
    ("variable.parameter", Color::Reset),
];

/// Parses one fenced block, a line at a time, carrying the state between them —
/// a string opened on one line is still a string on the next.
pub struct Highlighter {
    state: ParseState,
    stack: ScopeStack,
}

impl Highlighter {
    /// A highlighter for a fence's info string (`rust`, `py`, `Dockerfile`), or
    /// nothing if it names no language this build knows.
    pub fn new(info: &str) -> Option<Self> {
        let token = info.split_whitespace().next()?;
        let syntax = syntaxes().find_syntax_by_token(token)?;
        Some(Self {
            state: ParseState::new(syntax),
            stack: ScopeStack::new(),
        })
    }

    /// One line of code as styled runs. A line syntect cannot parse comes back
    /// as itself, unstyled.
    pub fn line(&mut self, text: &str) -> Vec<(String, Style)> {
        let Ok(ops) = self.state.parse_line(text, syntaxes()) else {
            return vec![(text.to_owned(), Style::default())];
        };
        let mut runs: Vec<(String, Style)> = Vec::new();
        for (piece, op) in ScopeRegionIterator::new(&ops, text) {
            // The op opens the region it is paired with, so it applies before
            // the piece is styled rather than after. Reversed, every run takes
            // the colour of the one before it — `fn` plain and the space after
            // it a keyword.
            if self.stack.apply(op).is_err() {
                return vec![(text.to_owned(), Style::default())];
            }
            if piece.is_empty() {
                continue;
            }
            let style = style_for(&self.stack);
            match runs.last_mut() {
                Some((text, last)) if *last == style => text.push_str(piece),
                _ => runs.push((piece.to_owned(), style)),
            }
        }
        if runs.is_empty() {
            runs.push((text.to_owned(), Style::default()));
        }
        runs
    }
}

fn style_for(stack: &ScopeStack) -> Style {
    for scope in stack.as_slice().iter().rev() {
        let name = scope.build_string();
        for (prefix, colour) in SCOPES {
            if name.starts_with(prefix) {
                let style = Style::default().fg(colour);
                return if prefix == "comment" {
                    style.add_modifier(Modifier::ITALIC)
                } else {
                    style
                };
            }
        }
    }
    Style::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(language: &str, line: &str) -> Vec<(String, Style)> {
        Highlighter::new(language)
            .expect("a known language")
            .line(line)
    }

    fn coloured(runs: &[(String, Style)], text: &str) -> Option<Color> {
        runs.iter()
            .find(|(t, _)| t.trim() == text)
            .and_then(|(_, s)| s.fg)
    }

    #[test]
    fn a_keyword_and_a_string_are_told_apart() {
        let runs = runs("rust", r#"fn main() { let s = "hi"; }"#);
        assert_eq!(coloured(&runs, "fn"), Some(Color::Magenta));
        assert_eq!(coloured(&runs, "\"hi\""), Some(Color::Green));
    }

    #[test]
    fn a_comment_is_dim_and_italic() {
        let runs = runs("rust", "// a note");
        let comment = runs
            .iter()
            .find(|(t, _)| t.contains("a note"))
            .expect("the comment");
        assert_eq!(comment.1.fg, Some(Color::DarkGray));
        assert!(comment.1.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn a_string_that_opens_on_one_line_is_still_a_string_on_the_next() {
        // The state carried between lines is the whole reason this is not a
        // function: without it every line of a block comment is code again.
        let mut highlighter = Highlighter::new("rust").expect("rust");
        highlighter.line("/* opened here");
        let runs = highlighter.line("still inside the comment");
        assert_eq!(runs[0].1.fg, Some(Color::DarkGray));
    }

    #[test]
    fn the_text_always_survives_whatever_the_parser_made_of_it() {
        for (language, line) in [
            ("rust", "fn main() {}"),
            ("python", "def f(x): return x  # hm"),
            ("toml", "key = \"value\""),
            ("json", "{\"a\": [1, 2]}"),
        ] {
            let joined: String = runs(language, line).into_iter().map(|(t, _)| t).collect();
            assert_eq!(joined, line, "{language}");
        }
    }

    #[test]
    fn a_language_nobody_has_heard_of_is_not_highlighted() {
        assert!(Highlighter::new("nonesuch").is_none());
        assert!(Highlighter::new("").is_none());
    }
}
