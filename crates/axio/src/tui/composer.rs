//! The line editor, with no terminal in it.
//!
//! Everything here is a pure function of keystrokes and text, so the editing
//! rules — where a word ends, what recalls history, what a paste does to a
//! selection that is really just a cursor — are testable without raw mode. The
//! surface owns the modal keys (interrupt, leave, approve); this owns the ones
//! that only mean something to text.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a keystroke asked the surface to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    /// Handled, or ignored — either way the surface has nothing to do.
    None,
    /// Send this, and keep it in history.
    Submit(String),
}

#[derive(Debug, Default)]
pub struct Composer {
    text: String,
    /// A character index, not a byte offset: every movement here is in units
    /// the user can see, and a multi-byte character is one of them.
    cursor: usize,
    history: Vec<String>,
    /// How far back through history the user has walked, from the end.
    recall: Option<usize>,
    /// What was being written when the walk started, so leaving history the
    /// way it was entered gives the draft back rather than an empty line.
    draft: String,
}

impl Composer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.recall = None;
    }

    /// Insert pasted text verbatim, newlines included. A paste is the ordinary
    /// way a multi-line prompt arrives, and splitting it into keystrokes would
    /// submit the first line and type the rest into whatever came next.
    pub fn paste(&mut self, text: &str) {
        let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, &normalised);
        self.cursor += normalised.chars().count();
        self.recall = None;
    }

    pub fn key(&mut self, key: KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let word = ctrl || alt;

        match key.code {
            // A terminal that reports the modifier gives multi-line entry from
            // the obvious key. One that does not still has Ctrl-J, which is
            // what a newline has always been on the wire, and paste.
            KeyCode::Enter if shift || alt => self.insert('\n'),
            KeyCode::Char('j') if ctrl => self.insert('\n'),
            KeyCode::Enter => {
                let text = self.text.trim().to_owned();
                if text.is_empty() {
                    return Edit::None;
                }
                self.remember(&text);
                self.clear();
                return Edit::Submit(text);
            }

            KeyCode::Char('w') if ctrl => self.delete_word_back(),
            KeyCode::Char('u') if ctrl => self.delete_to_line_start(),
            KeyCode::Char('k') if ctrl => self.delete_to_line_end(),
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Char(c) => self.insert(c),

            KeyCode::Backspace if word => self.delete_word_back(),
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let at = self.byte_at(self.cursor - 1);
                    self.text.remove(at);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let at = self.byte_at(self.cursor);
                if at < self.text.len() {
                    self.text.remove(at);
                }
            }

            KeyCode::Left if word => self.cursor = self.word_back(),
            KeyCode::Right if word => self.cursor = self.word_forward(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),

            // Up and Down are two keys with one binding each until the text has
            // more than one line in it, at which point moving inside what you
            // are writing has to win over walking away from it.
            KeyCode::Up if self.line_index() > 0 => self.move_line(-1),
            KeyCode::Down if self.line_index() + 1 < self.line_count() => self.move_line(1),
            KeyCode::Up => self.walk(-1),
            KeyCode::Down => self.walk(1),

            _ => {}
        }
        Edit::None
    }

    /// The text as it should be drawn, wrapped to `width`, with the cursor's
    /// position among those rows.
    pub fn rows(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows: Vec<String> = Vec::new();
        let mut at = (0usize, 0usize);
        let mut seen = 0usize;

        for line in self.text.split('\n') {
            let chars: Vec<char> = line.chars().collect();
            let start = rows.len();
            if chars.is_empty() {
                rows.push(String::new());
            } else {
                for chunk in chars.chunks(width) {
                    rows.push(chunk.iter().collect());
                }
            }
            // The cursor sits in this line if it has not already been placed
            // and does not run past the line's own end.
            if self.cursor >= seen && self.cursor <= seen + chars.len() {
                let offset = self.cursor - seen;
                at = (start + offset / width, offset % width);
            }
            seen += chars.len() + 1;
        }
        if rows.is_empty() {
            rows.push(String::new());
        }
        (rows, at)
    }

    fn remember(&mut self, text: &str) {
        if self.history.last().map(String::as_str) != Some(text) {
            self.history.push(text.to_owned());
        }
    }

    /// Walk history, keeping the draft the walk started from.
    fn walk(&mut self, direction: i32) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        let next = match (self.recall, direction) {
            (None, -1) => {
                self.draft = std::mem::take(&mut self.text);
                Some(last)
            }
            (Some(0), -1) => Some(0),
            (Some(i), -1) => Some(i - 1),
            (Some(i), 1) if i >= last => None,
            (Some(i), 1) => Some(i + 1),
            (None, _) => return,
            _ => return,
        };
        self.text = match next {
            Some(i) => self.history[i].clone(),
            None => std::mem::take(&mut self.draft),
        };
        self.recall = next;
        self.cursor = self.len();
    }

    fn insert(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
        self.recall = None;
    }

    fn delete_word_back(&mut self) {
        let to = self.word_back();
        if to < self.cursor {
            let (from, until) = (self.byte_at(to), self.byte_at(self.cursor));
            self.text.replace_range(from..until, "");
            self.cursor = to;
        }
    }

    fn delete_to_line_start(&mut self) {
        let start = self.line_start();
        if start < self.cursor {
            let (from, until) = (self.byte_at(start), self.byte_at(self.cursor));
            self.text.replace_range(from..until, "");
            self.cursor = start;
        }
    }

    fn delete_to_line_end(&mut self) {
        let end = self.line_end();
        if end > self.cursor {
            let (from, until) = (self.byte_at(self.cursor), self.byte_at(end));
            self.text.replace_range(from..until, "");
        }
    }

    // ------------------------------------------------------------- positions

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    fn byte_at(&self, index: usize) -> usize {
        self.text
            .char_indices()
            .nth(index)
            .map(|(at, _)| at)
            .unwrap_or(self.text.len())
    }

    /// A word ends where whitespace begins, and leading whitespace belongs to
    /// the word behind it — otherwise deleting a word at the start of a line
    /// deletes only the space.
    fn word_back(&self) -> usize {
        let chars = self.chars();
        let mut at = self.cursor;
        while at > 0 && chars[at - 1].is_whitespace() {
            at -= 1;
        }
        while at > 0 && !chars[at - 1].is_whitespace() {
            at -= 1;
        }
        at
    }

    fn word_forward(&self) -> usize {
        let chars = self.chars();
        let mut at = self.cursor;
        while at < chars.len() && chars[at].is_whitespace() {
            at += 1;
        }
        while at < chars.len() && !chars[at].is_whitespace() {
            at += 1;
        }
        at
    }

    fn line_start(&self) -> usize {
        let chars = self.chars();
        let mut at = self.cursor;
        while at > 0 && chars[at - 1] != '\n' {
            at -= 1;
        }
        at
    }

    fn line_end(&self) -> usize {
        let chars = self.chars();
        let mut at = self.cursor;
        while at < chars.len() && chars[at] != '\n' {
            at += 1;
        }
        at
    }

    fn line_index(&self) -> usize {
        self.chars()[..self.cursor]
            .iter()
            .filter(|c| **c == '\n')
            .count()
    }

    fn line_count(&self) -> usize {
        self.text.chars().filter(|c| *c == '\n').count() + 1
    }

    /// Move a line up or down, keeping the column where it can be kept.
    fn move_line(&mut self, direction: i32) {
        let column = self.cursor - self.line_start();
        let lines: Vec<&str> = self.text.split('\n').collect();
        let index = self.line_index();
        let target = if direction < 0 {
            index.saturating_sub(1)
        } else {
            (index + 1).min(lines.len() - 1)
        };
        let mut start = 0usize;
        for line in &lines[..target] {
            start += line.chars().count() + 1;
        }
        self.cursor = start + column.min(lines[target].chars().count());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn typed(text: &str) -> Composer {
        let mut c = Composer::default();
        for ch in text.chars() {
            c.key(key(KeyCode::Char(ch)));
        }
        c
    }

    #[test]
    fn enter_submits_and_clears() {
        let mut c = typed("hello");
        assert_eq!(c.key(key(KeyCode::Enter)), Edit::Submit("hello".into()));
        assert!(c.is_empty());
    }

    #[test]
    fn an_empty_line_submits_nothing() {
        let mut c = typed("   ");
        assert_eq!(c.key(key(KeyCode::Enter)), Edit::None);
    }

    #[test]
    fn a_modified_enter_makes_a_new_line_rather_than_sending() {
        for modifier in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
            let mut c = typed("first");
            assert_eq!(c.key(with(KeyCode::Enter, modifier)), Edit::None);
            for ch in "second".chars() {
                c.key(key(KeyCode::Char(ch)));
            }
            assert_eq!(c.text(), "first\nsecond");
        }
    }

    #[test]
    fn control_j_is_a_newline_for_terminals_that_report_no_modifier() {
        let mut c = typed("first");
        c.key(with(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(c.text(), "first\n");
    }

    #[test]
    fn a_multi_line_paste_stays_one_prompt() {
        // The failure this covers: a pasted shell session submitting its first
        // line as a prompt and typing the rest into whatever came next.
        let mut c = Composer::default();
        c.paste("one\r\ntwo\rthree");
        assert_eq!(c.text(), "one\ntwo\nthree");
        assert_eq!(
            c.key(key(KeyCode::Enter)),
            Edit::Submit("one\ntwo\nthree".into())
        );
    }

    #[test]
    fn a_paste_lands_at_the_cursor() {
        let mut c = typed("ac");
        c.key(key(KeyCode::Left));
        c.paste("b");
        assert_eq!(c.text(), "abc");
    }

    #[test]
    fn control_w_deletes_a_word_and_the_space_before_it() {
        let mut c = typed("one two three");
        c.key(with(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(c.text(), "one two ");
        c.key(with(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(c.text(), "one ");
    }

    #[test]
    fn control_u_and_control_k_cut_to_the_ends_of_the_line() {
        let mut c = typed("keep this");
        for _ in 0..4 {
            c.key(key(KeyCode::Left));
        }
        c.key(with(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(c.text(), "this");
        c.key(with(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(c.text(), "");
    }

    #[test]
    fn a_word_jump_moves_by_words_in_both_directions() {
        let mut c = typed("alpha beta gamma");
        c.key(with(KeyCode::Left, KeyModifiers::CONTROL));
        c.key(with(KeyCode::Left, KeyModifiers::CONTROL));
        c.key(key(KeyCode::Char('!')));
        assert_eq!(c.text(), "alpha !beta gamma");
        c.key(with(KeyCode::Right, KeyModifiers::CONTROL));
        c.key(key(KeyCode::Char('?')));
        assert_eq!(c.text(), "alpha !beta? gamma");
    }

    #[test]
    fn history_walks_back_and_returns_the_draft() {
        let mut c = typed("first");
        c.key(key(KeyCode::Enter));
        let mut c = {
            let mut next = c;
            for ch in "second".chars() {
                next.key(key(KeyCode::Char(ch)));
            }
            next.key(key(KeyCode::Enter));
            next
        };
        for ch in "draft".chars() {
            c.key(key(KeyCode::Char(ch)));
        }
        c.key(key(KeyCode::Up));
        assert_eq!(c.text(), "second");
        c.key(key(KeyCode::Up));
        assert_eq!(c.text(), "first");
        c.key(key(KeyCode::Down));
        assert_eq!(c.text(), "second");
        c.key(key(KeyCode::Down));
        assert_eq!(c.text(), "draft");
    }

    #[test]
    fn up_moves_within_the_text_before_it_reaches_for_history() {
        let mut c = typed("done");
        c.key(key(KeyCode::Enter));
        c.paste("one\ntwo");
        c.key(key(KeyCode::Up));
        // Still editing: the second line's cursor moved to the first line,
        // rather than the whole draft being replaced by history.
        assert_eq!(c.text(), "one\ntwo");
        c.key(key(KeyCode::Char('!')));
        assert_eq!(c.text(), "one!\ntwo");
    }

    #[test]
    fn wrapping_reports_where_the_cursor_is() {
        let mut c = Composer::default();
        c.paste("abcdef\nxy");
        let (rows, at) = c.rows(4);
        assert_eq!(rows, vec!["abcd", "ef", "xy"]);
        assert_eq!(at, (2, 2));

        // Back over "xy" and the newline: the cursor lands at the end of the
        // first logical line, which is the second row it wrapped onto.
        for _ in 0..3 {
            c.key(key(KeyCode::Left));
        }
        let (_, at) = c.rows(4);
        assert_eq!(at, (1, 2));
    }

    #[test]
    fn an_empty_composer_still_has_a_row_to_put_the_cursor_on() {
        let (rows, at) = Composer::default().rows(10);
        assert_eq!(rows, vec![""]);
        assert_eq!(at, (0, 0));
    }

    #[test]
    fn a_multi_byte_character_is_one_movement() {
        let mut c = typed("héllo");
        c.key(key(KeyCode::Backspace));
        c.key(key(KeyCode::Backspace));
        c.key(key(KeyCode::Backspace));
        assert_eq!(c.text(), "hé");
        c.key(key(KeyCode::Backspace));
        assert_eq!(c.text(), "h");
    }
}
