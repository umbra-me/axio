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
    /// Slash command names, without the slash, and what each does.
    pub commands: Vec<(String, String)>,
    /// Position in the slash menu's matches.
    menu_selected: usize,
    /// Esc closed the slash menu; it opens again when the text changes.
    menu_closed: bool,
    pub color: bool,
    /// Where `@path` completion looks.
    pub cwd: Option<std::path::PathBuf>,
    search: Option<Search>,
    /// A dim status line under the input, hidden while the command menu
    /// is open.
    pub footer: String,
    /// When Esc was last pressed on an empty prompt: twice opens /rewind.
    last_esc: Option<std::time::Instant>,
}

/// Ctrl-R: the typed query and the history entry it matched.
struct Search {
    query: String,
    hit: Option<usize>,
    /// The text being edited before the search began.
    saved: String,
}

/// A pasted path to an image file, as dropping a file on a terminal pastes
/// it (quoted, or with spaces escaped), turned into an `@` mention.
fn dropped_image(pasted: &str, cwd: Option<&std::path::Path>) -> Option<String> {
    let t = pasted.trim();
    if t.contains('\n') {
        return None;
    }
    let quoted = t.len() > 1
        && (t.starts_with('\'') && t.ends_with('\'') || t.starts_with('"') && t.ends_with('"'));
    let raw = if quoted { &t[1..t.len() - 1] } else { t };
    let mut path = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if !quoted => path.extend(chars.next()),
            c => path.push(c),
        }
    }
    let full = match path.strip_prefix("~/") {
        Some(rest) => std::path::PathBuf::from(std::env::var_os("HOME")?).join(rest),
        None => cwd.map_or_else(|| path.clone().into(), |d| d.join(&path)),
    };
    (full.is_file() && crate::context::image_type(&full).is_some())
        .then(|| format!("@{} ", path.replace(' ', "\\ ")))
}

/// Completes `partial`, a path relative to `cwd`: the whole name when one
/// entry matches (with `/` for a directory, a space for a file), else the
/// part all matches share. Hidden entries only when asked for with a dot.
fn complete_path(cwd: &std::path::Path, partial: &str) -> Option<String> {
    let (dir, prefix) = match partial.rsplit_once('/') {
        Some((d, p)) => (format!("{d}/"), p),
        None => (String::new(), partial),
    };
    let mut names: Vec<(String, bool)> = std::fs::read_dir(cwd.join(&dir))
        .ok()?
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.path().is_dir(),
            )
        })
        .filter(|(n, _)| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort();
    match names.as_slice() {
        [] => None,
        [(name, is_dir)] => Some(format!("{dir}{name}{}", if *is_dir { "/" } else { " " })),
        [(first, _), rest @ ..] => {
            let shared: String = first
                .chars()
                .enumerate()
                .take_while(|(i, c)| rest.iter().all(|(n, _)| n.chars().nth(*i) == Some(*c)))
                .map(|(_, c)| c)
                .collect();
            Some(format!("{dir}{shared}"))
        }
    }
}

impl Editor {
    pub fn text(&self) -> String {
        self.buf.iter().collect()
    }

    /// Puts text in the prompt to edit, as if typed.
    pub fn prefill(&mut self, text: &str) {
        self.browsing = None;
        self.set(text);
    }

    fn set(&mut self, text: &str) {
        self.buf = text.chars().collect();
        self.cursor = self.buf.len();
        self.menu_selected = 0;
        self.menu_closed = false;
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
        let start = self.buf[..self.cursor]
            .iter()
            .rposition(|c| c.is_whitespace())
            .map_or(0, |i| i + 1);
        let word: String = self.buf[start..self.cursor].iter().collect();
        if let Some(partial) = word.strip_prefix('@') {
            if let Some(done) = self
                .cwd
                .as_deref()
                .and_then(|cwd| complete_path(cwd, partial))
            {
                self.buf.splice(start + 1..self.cursor, done.chars());
                self.cursor = start + 1 + done.chars().count();
            }
            return;
        }
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
            .map(|(c, _)| c)
            .filter(|c| c.starts_with(prefix))
            .collect();
        match matches.as_slice() {
            [] => {}
            [one] => self.set(&format!("/{one} ")),
            [first, rest @ ..] => {
                // Compare characters, so the prefix never ends inside one.
                let shared: String = first
                    .chars()
                    .enumerate()
                    .take_while(|(i, c)| rest.iter().all(|m| m.chars().nth(*i) == Some(*c)))
                    .map(|(_, c)| c)
                    .collect();
                self.set(&format!("/{shared}"));
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

    /// Commands matching a `/name` being typed: names that start with it,
    /// then names that contain it. Empty when the menu is closed.
    fn menu(&self) -> Vec<usize> {
        // A recalled history entry is not being typed, so Up and Down keep
        // walking history through `/command` entries.
        if self.menu_closed
            || self.search.is_some()
            || self.browsing.is_some()
            || self.cursor != self.buf.len()
        {
            return Vec::new();
        }
        let text = self.text();
        let Some(typed) = text.strip_prefix('/') else {
            return Vec::new();
        };
        if typed.contains(char::is_whitespace) {
            return Vec::new();
        }
        let typed = typed.to_lowercase();
        let name = |i: &usize| self.commands[*i].0.to_lowercase();
        let exact = |i: &usize| name(i) == typed;
        let starts = |i: &usize| !exact(i) && name(i).starts_with(&typed);
        let inside = |i: &usize| !name(i).starts_with(&typed) && name(i).contains(&typed);
        let all = 0..self.commands.len();
        all.clone()
            .filter(exact)
            .chain(all.clone().filter(starts))
            .chain(all.filter(inside))
            .collect()
    }

    /// Keys the slash menu takes while it is open.
    fn menu_key(&mut self, key: &Key) -> Option<Outcome> {
        let menu = self.menu();
        let chosen = menu.get(self.menu_selected.min(menu.len().saturating_sub(1)))?;
        let name = self.commands[*chosen].0.clone();
        match key {
            Key::Up => self.menu_selected = self.menu_selected.saturating_sub(1),
            Key::Down => self.menu_selected = (self.menu_selected + 1).min(menu.len() - 1),
            Key::Ctrl('i') => self.set(&format!("/{name} ")),
            Key::Esc => self.menu_closed = true,
            Key::Enter => {
                self.set(&format!("/{name}"));
                return None;
            }
            _ => return None,
        }
        Some(Outcome::Editing)
    }

    pub fn handle(&mut self, key: Key) -> Outcome {
        let before = self.text();
        if let Some(outcome) = self.menu_key(&key) {
            return outcome;
        }
        // Editing a recalled entry makes it the text being typed.
        if self.browsing.is_some() && !matches!(key, Key::Up | Key::Down) {
            let edits = matches!(
                key,
                Key::Char(_)
                    | Key::Paste(_)
                    | Key::Backspace
                    | Key::Delete
                    | Key::AltBackspace
                    | Key::Ctrl('w' | 'u' | 'k')
            );
            if edits {
                self.browsing = None;
            }
        }
        let outcome = self.edit(key);
        if self.text() != before {
            self.menu_selected = 0;
            self.menu_closed = false;
        }
        outcome
    }

    fn edit(&mut self, key: Key) -> Outcome {
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
            Key::Esc if self.buf.is_empty() => {
                let twice = self
                    .last_esc
                    .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(800));
                self.last_esc = (!twice).then(std::time::Instant::now);
                if twice {
                    return Outcome::Submit("/rewind".into());
                }
            }
            Key::Ctrl('i') => self.complete(),
            Key::Ctrl('o') if self.buf.is_empty() => return Outcome::Submit("/output".into()),
            Key::Char(c) => self.insert(&c.to_string()),
            Key::Paste(s) => {
                let text = dropped_image(&s, self.cwd.as_deref())
                    // Tabs would render at a width the layout cannot know.
                    .unwrap_or_else(|| s.replace('\t', "    "));
                self.insert(&text);
            }
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
            let mut used = 0;
            let line: String = format!("search history: {}  › {found}", s.query)
                .chars()
                .take_while(|c| {
                    used += crate::term::char_width(*c);
                    used < width.max(4)
                })
                .collect();
            let col = crate::term::text_width(&line);
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
            let w = crate::term::char_width(*c);
            // Wrap first, so a cursor on the wrapped character sits on the new row.
            if *c != '\n' && col + w > width {
                out.push('\n');
                row += 1;
                col = 0;
            }
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
            out.push(*c);
            col += w;
        }
        if self.cursor == self.buf.len() {
            cursor = (row, col);
        }
        let menu = self.menu();
        // A full last row leaves the terminal's cursor in a pending-wrap state;
        // move it to the next row so the arithmetic below stays exact.
        if col == width {
            out.push('\n');
            row += 1;
            if cursor == (row - 1, width) {
                cursor = (row, 0);
            }
        }
        if !menu.is_empty() {
            if col < width {
                out.push('\n');
                row += 1;
            }
            let rows = self.menu_rows(&menu, width);
            row += rows.len() - 1;
            out.push_str(&rows.join("\n"));
        } else if !self.footer.is_empty() {
            if col < width {
                out.push('\n');
                row += 1;
            }
            let line: String = self.footer.chars().take(width - 1).collect();
            out.push_str(&crate::ui::paint(self.color, crate::ui::DIM, &line));
        }
        Layout {
            text: out,
            cursor_row: cursor.0,
            cursor_col: cursor.1,
            end_row: row,
        }
    }

    /// Up to eight matches around the selected one, each fitting one row.
    fn menu_rows(&self, menu: &[usize], width: usize) -> Vec<String> {
        const SHOWN: usize = 8;
        let selected = self.menu_selected.min(menu.len() - 1);
        let top = selected.saturating_sub(SHOWN - 1);
        let name_width = menu
            .iter()
            .map(|&i| self.commands[i].0.chars().count() + 1)
            .max()
            .unwrap_or(0)
            .min(24);
        let paint = |code: &str, s: &str| crate::ui::paint(self.color, code, s);
        let mut rows = Vec::new();
        for (n, &i) in menu.iter().enumerate().skip(top).take(SHOWN) {
            let (name, about) = &self.commands[i];
            let name = format!("/{name}");
            let room = width.saturating_sub(4 + name_width.max(name.chars().count()) + 1);
            let about: String = about
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(room)
                .collect();
            let name = format!("{name:<name_width$}");
            rows.push(if n == selected {
                format!(
                    "{} {}  {}",
                    paint("\x1b[1;36m", "›"),
                    paint("\x1b[1m", &name),
                    paint(crate::ui::DIM, &about)
                )
            } else {
                format!("  {name}  {}", paint(crate::ui::DIM, &about))
            });
        }
        if menu.len() > SHOWN {
            rows.push(paint(
                crate::ui::DIM,
                &format!("  {} of {} · Tab completes", selected + 1, menu.len()),
            ));
        }
        rows
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
    fn wide_characters_take_two_columns() {
        let mut e = Editor::default();
        e.handle(Key::Paste("日本語".into()));
        // "› " is 2 columns, so at width 7 the third character wraps.
        let l = e.layout(7);
        assert_eq!(l.text, "› 日本\n語");
        assert_eq!((l.cursor_row, l.cursor_col, l.end_row), (1, 2, 1));
        e.handle(Key::Home);
        e.handle(Key::Right);
        e.handle(Key::Right);
        let l = e.layout(7);
        assert_eq!(
            (l.cursor_row, l.cursor_col),
            (1, 0),
            "cursor on a wrapped character is on its row"
        );
        e.handle(Key::Paste("\tx".into()));
        assert!(e.text().contains("    x"), "pasted tabs become spaces");
    }

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
    fn dropped_image_paths_become_mentions() {
        let dir = std::env::temp_dir().join(format!("axio-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Shot 1.png"), "x").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let abs = dir.join("Shot 1.png").display().to_string();
        let mut e = Editor {
            cwd: Some(dir.clone()),
            ..Editor::default()
        };
        e.handle(Key::Paste(abs.replace(' ', "\\ ")));
        assert_eq!(e.text(), format!("@{} ", abs.replace(' ', "\\ ")));
        e.set("");
        e.handle(Key::Paste(format!("'{abs}'")));
        assert_eq!(
            e.text(),
            format!("@{} ", abs.replace(' ', "\\ ")),
            "quoted form too"
        );
        e.set("");
        e.handle(Key::Paste(dir.join("notes.txt").display().to_string()));
        assert!(!e.text().starts_with('@'), "only images");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tab_completes_mentioned_paths() {
        let dir = std::env::temp_dir().join(format!("axio-complete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        for f in ["src/main.rs", "src/markdown.rs", ".hidden"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let mut e = Editor {
            cwd: Some(dir.clone()),
            ..Editor::default()
        };
        e.handle(Key::Paste("look at @sr".into()));
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "look at @src/");
        e.handle(Key::Char('m'));
        e.handle(Key::Ctrl('i'));
        assert_eq!(
            e.text(),
            "look at @src/ma",
            "shared part of main.rs and markdown.rs"
        );
        e.handle(Key::Char('i'));
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "look at @src/main.rs ");
        e.set("@.h");
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "@.hidden ");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tab_completes_slash_commands() {
        let mut e = Editor {
            commands: ["compact", "commit", "help"]
                .map(|c| (c.to_string(), String::new()))
                .into(),
            ..Editor::default()
        };
        e.handle(Key::Paste("/h".into()));
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/help ");
        e.set("/co");
        e.handle(Key::Down);
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/commit ", "Tab takes the menu's choice");
        e.set("/co");
        e.handle(Key::Esc);
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/com", "with the menu closed, the shared part");
        e.handle(Key::Char('p'));
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/compact ");
        e.set("plain");
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "plain");
        e.commands = ["café", "cafè"]
            .map(|c| (c.to_string(), String::new()))
            .into();
        e.set("/ca");
        e.handle(Key::Esc);
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/caf", "stops before characters that differ");
    }

    fn with_commands() -> Editor {
        Editor {
            commands: [
                ("model", "pick the model"),
                ("mcp-tools", "a command"),
                ("new", "start a new session"),
                ("session", "show the session file"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .into(),
            history: vec!["earlier".into()],
            ..Editor::default()
        }
    }

    #[test]
    fn slash_shows_a_menu_that_enter_runs() {
        let mut e = with_commands();
        typed(&mut e, "/m");
        let shown = e.layout(80);
        assert!(shown.text.contains("/model") && shown.text.contains("pick the model"));
        assert!(shown.text.contains("/mcp-tools"));
        assert!(!shown.text.contains("/new"));
        assert_eq!(
            (shown.cursor_row, shown.end_row),
            (0, 2),
            "menu below the prompt"
        );
        e.handle(Key::Down);
        assert_eq!(e.handle(Key::Enter), Outcome::Submit("/mcp-tools".into()));
        assert!(!e.layout(80).text.contains("/model"), "gone after sending");
    }

    #[test]
    fn slash_menu_matches_inside_names_after_prefixes() {
        let mut e = with_commands();
        typed(&mut e, "/s");
        let text = e.layout(80).text;
        let (session, tools) = (
            text.find("/session").unwrap(),
            text.find("/mcp-tools").unwrap(),
        );
        assert!(session < tools, "prefix matches first: {text}");
        e.handle(Key::Ctrl('i'));
        assert_eq!(e.text(), "/session ");
        assert!(
            !e.layout(80).text.contains("show the session"),
            "closed once an argument starts"
        );
    }

    #[test]
    fn the_status_line_sits_under_the_input() {
        let mut e = with_commands();
        e.footer = "opus · 4% context · ~$0.12".into();
        typed(&mut e, "hello");
        let l = e.layout(80);
        assert_eq!(l.text, "› hello\nopus · 4% context · ~$0.12");
        assert_eq!((l.cursor_row, l.cursor_col, l.end_row), (0, 7, 1));
        e.set("");
        typed(&mut e, "/ne");
        assert!(
            !e.layout(80).text.contains("context"),
            "the command menu replaces it"
        );
        e.footer = "x".repeat(100);
        e.set("");
        assert_eq!(
            e.layout(40).text.lines().last().unwrap().chars().count(),
            39,
            "fits the width"
        );
    }

    #[test]
    fn ctrl_o_on_an_empty_prompt_shows_the_last_output() {
        let mut e = Editor::default();
        assert_eq!(e.handle(Key::Ctrl('o')), Outcome::Submit("/output".into()));
        assert!(e.history.is_empty(), "not kept in history");
        typed(&mut e, "text");
        assert_eq!(e.handle(Key::Ctrl('o')), Outcome::Editing);
    }

    #[test]
    fn esc_twice_on_an_empty_prompt_opens_rewind() {
        let mut e = Editor::default();
        assert_eq!(e.handle(Key::Esc), Outcome::Editing);
        assert_eq!(e.handle(Key::Esc), Outcome::Submit("/rewind".into()));
        assert!(e.history.is_empty());
        typed(&mut e, "x");
        e.handle(Key::Esc);
        assert_eq!(
            e.handle(Key::Esc),
            Outcome::Editing,
            "not while there is text"
        );
        e.prefill("edit me");
        assert_eq!(e.text(), "edit me");
    }

    #[test]
    fn history_walks_past_slash_commands() {
        let mut e = with_commands();
        e.history = vec!["first".into(), "/new".into(), "last".into()];
        let walk = |e: &mut Editor, k: Key| {
            e.handle(k);
            e.text()
        };
        assert_eq!(walk(&mut e, Key::Up), "last");
        assert_eq!(walk(&mut e, Key::Up), "/new");
        assert!(
            !e.layout(80).text.contains("start a new"),
            "no menu over a recalled entry"
        );
        assert_eq!(walk(&mut e, Key::Up), "first", "Up keeps walking history");
        assert_eq!(walk(&mut e, Key::Down), "/new");
        assert_eq!(walk(&mut e, Key::Down), "last");
        walk(&mut e, Key::Up);
        typed(&mut e, " ");
        e.handle(Key::Backspace);
        assert_eq!(e.text(), "/new");
        assert!(
            e.layout(80).text.contains("start a new"),
            "editing brings the menu back"
        );
    }

    #[test]
    fn esc_closes_the_menu_until_the_text_changes() {
        let mut e = with_commands();
        typed(&mut e, "/n");
        e.handle(Key::Esc);
        assert!(!e.layout(80).text.contains("start a new"));
        assert_eq!(e.handle(Key::Up), Outcome::Editing);
        assert_eq!(e.text(), "earlier", "Up walks history with the menu closed");
        e.set("");
        typed(&mut e, "/ne");
        assert!(e.layout(80).text.contains("start a new"));
        assert_eq!(e.handle(Key::Enter), Outcome::Submit("/new".into()));
        e.commands.push(("ne".into(), "an exact name".into()));
        typed(&mut e, "/ne");
        assert_eq!(
            e.handle(Key::Enter),
            Outcome::Submit("/ne".into()),
            "an exact name comes first"
        );
        typed(&mut e, "/zzz");
        assert_eq!(
            e.handle(Key::Enter),
            Outcome::Submit("/zzz".into()),
            "no match sends as typed"
        );
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
