//! A small multi-line prompt editor: Enter sends, Alt-Enter or a trailing
//! backslash adds a line, Up and Down walk history, Ctrl-R searches it.

use crate::term::Key;

pub const PROMPT: &str = "› ";
const CONTINUATION: &str = "  ";

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Editing,
    Submit(String),
    /// Ctrl-C on an empty line.
    Interrupt,
    /// Ctrl-D on an empty line.
    Exit,
    Redraw,
}

#[derive(Default)]
pub struct Editor {
    buf: Vec<char>,
    cursor: usize,
    pub history: Vec<String>,
    browsing: Option<usize>,
    draft: String,
    /// Slash command names, without the slash, for Tab completion.
    pub commands: Vec<String>,
    search: Option<Search>,
}

/// Ctrl-R: the typed query and the history entry it matched.
struct Search {
    query: String,
    hit: Option<usize>,
    /// The text being edited before the search began.
    saved: String,
}

impl Editor {
    pub fn text(&self) -> String {
        self.buf.iter().collect()
    }

    fn set(&mut self, text: &str) {
        self.buf = text.chars().collect();
        self.cursor = self.buf.len();
    }

    fn line_start(&self, at: usize) -> usize {
        self.buf[..at]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.buf[at..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(self.buf.len(), |i| at + i)
    }

    fn word_left(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && self.buf[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.buf[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let mut i = self.cursor;
        while i < self.buf.len() && self.buf[i].is_whitespace() {
            i += 1;
        }
        while i < self.buf.len() && !self.buf[i].is_whitespace() {
            i += 1;
        }
        i
    }

    fn insert(&mut self, s: &str) {
        for c in s.chars() {
            self.buf.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// Moves between lines of the buffer, or through history at its edges.
    fn vertical(&mut self, up: bool) {
        let start = self.line_start(self.cursor);
        let col = self.cursor - start;
        if up && start > 0 {
            let prev = self.line_start(start - 1);
            self.cursor = (prev + col).min(start - 1);
            return;
        }
        let end = self.line_end(self.cursor);
        if !up && end < self.buf.len() {
            let next_end = self.line_end(end + 1);
            self.cursor = (end + 1 + col).min(next_end);
            return;
        }
        self.history_step(up);
    }

    fn history_step(&mut self, up: bool) {
        let next = match (self.browsing, up) {
            (None, true) if !self.history.is_empty() => {
                self.draft = self.text();
                Some(self.history.len() - 1)
            }
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), false) => None,
            _ => return,
        };
        self.browsing = next;
        let text = match next {
            Some(i) => self.history[i].clone(),
            None => std::mem::take(&mut self.draft),
        };
        self.set(&text);
    }

    /// Completes a slash command name at the end of the line: the whole
    /// name when one matches, else the part all matches share.
    fn complete(&mut self) {
        let text = self.text();
        let Some(prefix) = text.strip_prefix('/') else {
            return;
        };
        if self.cursor != self.buf.len() || prefix.contains(char::is_whitespace) {
            return;
        }
        let matches: Vec<&String> = self
            .commands
            .iter()
            .filter(|c| c.starts_with(prefix))
            .collect();
        match matches.as_slice() {
            [] => {}
            [one] => self.set(&format!("/{one} ")),
            [first, rest @ ..] => {
                let mut common = first.len();
                for m in rest {
                    common = common.min(
                        first
                            .bytes()
                            .zip(m.bytes())
                            .take_while(|(a, b)| a == b)
                            .count(),
                    );
                }
                self.set(&format!("/{}", &first[..common]));
            }
        }
    }

    /// The newest history entry before `before` that contains `query`.
    fn find(&self, query: &str, before: usize) -> Option<usize> {
        let query = query.to_lowercase();
        (0..before.min(self.history.len()))
            .rev()
            .find(|&i| self.history[i].to_lowercase().contains(&query))
    }

    /// Keys while searching. Returns the key to handle normally once the
    /// search has ended by accepting its match.
    fn search_key(&mut self, key: Key) -> Option<Key> {
        let s = self.search.as_mut()?;
        match key {
            Key::Char(_) | Key::Paste(_) => {
                match key {
                    Key::Char(c) => s.query.push(c),
                    Key::Paste(t) => s.query.push_str(t.lines().next().unwrap_or("")),
                    _ => unreachable!(),
                }
                let q = s.query.clone();
                let hit = self.find(&q, self.history.len());
                if let Some(s) = &mut self.search {
                    s.hit = hit;
                }
            }
            Key::Backspace => {
                s.query.pop();
                let q = s.query.clone();
                let hit = if q.is_empty() {
                    None
                } else {
                    self.find(&q, self.history.len())
                };
                if let Some(s) = &mut self.search {
                    s.hit = hit;
                }
            }
            Key::Ctrl('r') => {
                let (q, from) = (s.query.clone(), s.hit.unwrap_or(self.history.len()));
                if let (Some(hit), Some(s)) = (self.find(&q, from), &mut self.search) {
                    s.hit = Some(hit);
                }
            }
            Key::Esc | Key::Ctrl('g') | Key::Ctrl('c') => {
                let s = self.search.take()?;
                self.set(&s.saved);
            }
            other => {
                let s = self.search.take()?;
                match s.hit {
                    Some(i) => {
                        let text = self.history[i].clone();
                        self.set(&text);
                    }
                    None => self.set(&s.saved),
                }
                // Enter accepts the match for editing rather than sending it.
                return (other != Key::Enter).then_some(other);
            }
        }
        None
    }

    pub fn handle(&mut self, key: Key) -> Outcome {
        let key = if self.search.is_some() {
            match self.search_key(key) {
                Some(k) => k,
                None => return Outcome::Editing,
            }
        } else {
            key
        };
        match key {
            Key::Ctrl('r') => {
                self.search = Some(Search {
                    query: String::new(),
                    hit: None,
                    saved: self.text(),
                });
            }
            Key::Ctrl('i') => self.complete(),
            Key::Char(c) => self.insert(&c.to_string()),
            Key::Paste(s) => self.insert(&s),
            Key::AltEnter => self.insert("\n"),
            Key::Enter => {
                if self.cursor == self.buf.len() && self.buf.last() == Some(&'\\') {
                    self.buf.pop();
                    self.cursor -= 1;
                    self.insert("\n");
                } else {
                    let text = self.text();
                    self.buf.clear();
                    self.cursor = 0;
                    self.browsing = None;
                    if !text.trim().is_empty() && self.history.last() != Some(&text) {
                        self.history.push(text.clone());
                    }
                    return Outcome::Submit(text);
                }
            }
            Key::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.buf.remove(self.cursor);
            }
            Key::Delete if self.cursor < self.buf.len() => {
                self.buf.remove(self.cursor);
            }
            Key::AltBackspace | Key::Ctrl('w') => {
                let to = self.word_left();
                self.buf.drain(to..self.cursor);
                self.cursor = to;
            }
            Key::Ctrl('u') => {
                let start = self.line_start(self.cursor);
                self.buf.drain(start..self.cursor);
                self.cursor = start;
            }
            Key::Ctrl('k') => {
                let end = self.line_end(self.cursor);
                self.buf.drain(self.cursor..end);
            }
            Key::Left => self.cursor = self.cursor.saturating_sub(1),
            Key::Right => self.cursor = (self.cursor + 1).min(self.buf.len()),
            Key::WordLeft => self.cursor = self.word_left(),
            Key::WordRight => self.cursor = self.word_right(),
            Key::Home => self.cursor = self.line_start(self.cursor),
            Key::End => self.cursor = self.line_end(self.cursor),
            Key::Up => self.vertical(true),
            Key::Down => self.vertical(false),
            Key::Ctrl('c') => {
                if self.buf.is_empty() {
                    return Outcome::Interrupt;
                }
                self.buf.clear();
                self.cursor = 0;
                self.browsing = None;
            }
            Key::Ctrl('d') if self.buf.is_empty() => return Outcome::Exit,
            Key::Ctrl('l') => return Outcome::Redraw,
            _ => {}
        }
        Outcome::Editing
    }

    /// The text to print for the prompt area at `width` columns, plus the
    /// cursor's row and column and the row the printed text ends on.
    pub fn layout(&self, width: usize) -> Layout {
        if let Some(s) = &self.search {
            let found = match s.hit {
                Some(i) => self.history[i].lines().next().unwrap_or("").to_string(),
                None if s.query.is_empty() => String::new(),
                None => "no match".into(),
            };
            let line: String = format!("search history: {}  › {found}", s.query)
                .chars()
                .take(width.max(4) - 1)
                .collect();
            let col = line.chars().count();
            return Layout {
                text: line,
                cursor_row: 0,
                cursor_col: col,
                end_row: 0,
            };
        }
        let width = width.max(4);
        let mut out = String::from(PROMPT);
        let (mut row, mut col) = (0, PROMPT.chars().count());
        let mut cursor = (row, col);
        for (i, c) in self.buf.iter().enumerate() {
            if i == self.cursor {
                cursor = (row, col);
            }
            if *c == '\n' {
                out.push('\n');
                out.push_str(CONTINUATION);
                row += 1;
                col = CONTINUATION.len();
                continue;
            }
            if col == width {
                out.push('\n');
                row += 1;
                col = 0;
            }
            out.push(*c);
            col += 1;
        }
        if self.cursor == self.buf.len() {
            cursor = (row, col);
        }
        // A full last row leaves the terminal's cursor in a pending-wrap state;
        // move it to the next row so the arithmetic below stays exact.
        if col == width {
            out.push('\n');
            row += 1;
            if cursor == (row - 1, width) {
                cursor = (row, 0);
            }
        }
        Layout {
            text: out,
            cursor_row: cursor.0,
            cursor_col: cursor.1,
            end_row: row,
        }
    }
}

pub struct Layout {
    pub text: String,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub end_row: usize,
}

impl Layout {
    /// Escape codes that redraw this layout over one drawn with its cursor on
    /// `previous_cursor_row`.
    pub fn redraw(&self, previous_cursor_row: usize) -> String {
        let mut s = String::from("\r");
        if previous_cursor_row > 0 {
            s.push_str(&format!("\x1b[{previous_cursor_row}A"));
        }
        s.push_str("\x1b[J");
        s.push_str(&self.text);
        let up = self.end_row - self.cursor_row;
        if up > 0 {
            s.push_str(&format!("\x1b[{up}A"));
        }
        s.push('\r');
        if self.cursor_col > 0 {
            s.push_str(&format!("\x1b[{}C", self.cursor_col));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_r_searches_history_newest_first() {
        let mut e = Editor {
            history: ["cargo build", "git status", "cargo test"]
                .map(String::from)
                .into(),
            ..Editor::default()
        };
        e.handle(Key::Paste("draft".into()));
        e.handle(Key::Ctrl('r'));
        for c in "CARGO".chars() {
            e.handle(Key::Char(c));
        }
        assert!(e.layout(80).text.ends_with("› cargo test"));
        e.handle(Key::Ctrl('r'));
        assert!(e.layout(80).text.ends_with("› cargo build"));
        assert_eq!(
            e.handle(Key::Enter),
            Outcome::Editing,
            "Enter accepts for editing"
        );
        assert_eq!(e.text(), "cargo build");
        assert_eq!(e.handle(Key::Enter), Outcome::Submit("cargo build".into()));

        e.handle(Key::Paste("mine".into()));
        e.handle(Key::Ctrl('r'));
        e.handle(Key::Char('z'));
        assert!(e.layout(80).text.ends_with("no match"));
        e.handle(Key::Esc);
        assert_eq!(e.text(), "mine", "Esc restores the draft");

        e.handle(Key::Ctrl('r'));
        e.handle(Key::Paste("stat".into()));
        e.handle(Key::End);
        e.handle(Key::Char('!'));
        assert_eq!(e.text(), "git status!", "other keys accept and then apply");
    }

    #[test]
    fn tab_completes_slash_commands() {
        let mut e = Editor {
            commands: ["compact", "commit", "help"].map(String::from).into(),
            ..Editor::default()
        };
        e.handle(Key::Paste("/h".into()));
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/help ");
        e.set("/co");
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/com");
        e.handle(Key::Char('p'));
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/compact ");
        e.set("plain");
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "plain");
    }

    fn typed(e: &mut Editor, s: &str) {
        for c in s.chars() {
            e.handle(Key::Char(c));
        }
    }

    #[test]
    fn edits_and_submits() {
        let mut e = Editor::default();
        typed(&mut e, "hello wrld");
        e.handle(Key::WordLeft);
        e.handle(Key::Right);
        e.handle(Key::Char('o'));
        assert_eq!(e.text(), "hello world");
        e.handle(Key::Ctrl('w'));
        assert_eq!(e.text(), "hello rld");
        e.handle(Key::End);
        assert_eq!(e.handle(Key::Enter), Outcome::Submit("hello rld".into()));
        assert_eq!(e.text(), "");
    }

    #[test]
    fn backslash_or_alt_enter_adds_a_line() {
        let mut e = Editor::default();
        typed(&mut e, "one\\");
        assert_eq!(e.handle(Key::Enter), Outcome::Editing);
        typed(&mut e, "two");
        e.handle(Key::AltEnter);
        e.handle(Key::Paste("three\nfour".into()));
        assert_eq!(
            e.handle(Key::Enter),
            Outcome::Submit("one\ntwo\nthree\nfour".into())
        );
    }

    #[test]
    fn up_and_down_move_lines_then_history() {
        let mut e = Editor::default();
        typed(&mut e, "first");
        e.handle(Key::Enter);
        typed(&mut e, "ab\ncd");
        e.handle(Key::Up);
        e.handle(Key::Char('X'));
        assert_eq!(e.text(), "abX\ncd", "Up moves to the line above first");
        e.handle(Key::Up);
        assert_eq!(e.text(), "first");
        e.handle(Key::Down);
        assert_eq!(e.text(), "abX\ncd", "the draft comes back");
    }

    #[test]
    fn ctrl_c_clears_then_interrupts_and_ctrl_d_exits_when_empty() {
        let mut e = Editor::default();
        typed(&mut e, "x");
        assert_eq!(e.handle(Key::Ctrl('d')), Outcome::Editing);
        assert_eq!(e.handle(Key::Ctrl('c')), Outcome::Editing);
        assert_eq!(e.handle(Key::Ctrl('c')), Outcome::Interrupt);
        assert_eq!(e.handle(Key::Ctrl('d')), Outcome::Exit);
    }

    #[test]
    fn layout_wraps_and_places_the_cursor() {
        let mut e = Editor::default();
        typed(&mut e, "abcdefgh");
        let l = e.layout(6);
        assert_eq!(l.text, "› abcd\nefgh");
        assert_eq!((l.cursor_row, l.cursor_col, l.end_row), (1, 4, 1));
        e.handle(Key::Home);
        let l = e.layout(6);
        assert_eq!((l.cursor_row, l.cursor_col), (0, 2));
        let mut full = Editor::default();
        typed(&mut full, "abcd");
        let l = full.layout(6);
        assert_eq!(l.text, "› abcd\n");
        assert_eq!((l.cursor_row, l.cursor_col, l.end_row), (1, 0, 1));
    }

    #[test]
    fn layout_indents_continuation_lines() {
        let mut e = Editor::default();
        e.handle(Key::Paste("a\nb".into()));
        let l = e.layout(80);
        assert_eq!(l.text, "› a\n  b");
        assert_eq!((l.cursor_row, l.cursor_col), (1, 3));
    }
}
