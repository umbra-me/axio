//! Inline pickers and questions drawn below the prompt: type to filter, Up
//! and Down to move, Enter to choose, Esc to cancel.

use crate::term::{Key, Keys};
use std::io::Write;

const ROWS: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub detail: String,
}

impl Choice {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Choice {
        Choice {
            label: label.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Pending,
    Picked(usize),
    /// Enter on text no choice matches, when the picker allows that.
    Typed(String),
    Cancelled,
}

pub struct Picker {
    title: String,
    choices: Vec<Choice>,
    query: String,
    /// Position in the filtered list.
    selected: usize,
    top: usize,
    /// When set, Enter on text nothing matches gives that text, with this
    /// prefix added unless the text already holds a `/`.
    typed_prefix: Option<String>,
}

impl Picker {
    /// Starts on `current` when given, so Enter keeps what is in use.
    pub fn new(title: &str, choices: Vec<Choice>, current: Option<usize>) -> Picker {
        let mut p = Picker {
            title: title.into(),
            choices,
            query: String::new(),
            selected: 0,
            top: 0,
            typed_prefix: None,
        };
        if let Some(c) = current.filter(|c| *c < p.choices.len()) {
            p.selected = c;
            p.scroll();
        }
        p
    }

    /// Lets Enter use what was typed when no choice matches it, for a
    /// list that may not hold everything, such as a server's models.
    pub fn allow_typed(mut self, prefix: &str) -> Picker {
        self.typed_prefix = Some(prefix.into());
        self
    }

    /// What Enter would give for the typed text, when that is allowed.
    fn typed(&self) -> Option<String> {
        let q = self.query.trim();
        let prefix = self.typed_prefix.as_deref()?;
        match q {
            "" => None,
            q if q.contains('/') => Some(q.to_string()),
            q => Some(format!("{prefix}{q}")),
        }
    }

    /// Indexes of the choices whose label or detail holds every typed word.
    fn filtered(&self) -> Vec<usize> {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        (0..self.choices.len())
            .filter(|&i| {
                let c = &self.choices[i];
                let hay = format!("{} {}", c.label, c.detail).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect()
    }

    fn scroll(&mut self) {
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + ROWS {
            self.top = self.selected + 1 - ROWS;
        }
    }

    pub fn key(&mut self, key: Key) -> Step {
        let shown = self.filtered();
        match key {
            Key::Enter => {
                return match shown.get(self.selected) {
                    Some(&i) => Step::Picked(i),
                    None => self.typed().map_or(Step::Pending, Step::Typed),
                };
            }
            Key::Esc | Key::Ctrl('c') | Key::Ctrl('g') => return Step::Cancelled,
            Key::Up | Key::Ctrl('p') => self.selected = self.selected.saturating_sub(1),
            Key::Down | Key::Ctrl('n') => {
                self.selected = (self.selected + 1).min(shown.len().saturating_sub(1));
            }
            Key::Char(c) => self.set_query(format!("{}{c}", self.query)),
            Key::Paste(t) => self.set_query(format!("{}{}", self.query, t.trim())),
            Key::Backspace => {
                let mut q = self.query.clone();
                q.pop();
                self.set_query(q);
            }
            Key::Ctrl('u') => self.set_query(String::new()),
            _ => {}
        }
        self.scroll();
        Step::Pending
    }

    fn set_query(&mut self, q: String) {
        self.query = q;
        self.selected = 0;
        self.top = 0;
    }

    /// The rows to draw, each cut to fit `width` so none wraps.
    pub fn lines(&self, width: usize, color: bool) -> Vec<String> {
        let shown = self.filtered();
        let dim = |s: &str| crate::ui::paint(color, crate::ui::DIM, s);
        let mut out = vec![
            format!(
                "{}  {}",
                self.title,
                dim("type to filter · ↑↓ move · Enter choose · Esc cancel")
            ),
            format!("› {}", self.query),
        ];
        let label_width = shown
            .iter()
            .skip(self.top)
            .take(ROWS)
            .map(|&i| self.choices[i].label.chars().count())
            .max()
            .unwrap_or(0);
        for (row, &i) in shown.iter().enumerate().skip(self.top).take(ROWS) {
            let c = &self.choices[i];
            let label = fit(
                &format!("{:<label_width$}", c.label),
                width.saturating_sub(4),
            );
            let room = width.saturating_sub(4 + label.chars().count() + 2);
            let detail = fit(&c.detail, room);
            out.push(if row == self.selected {
                format!(
                    "{} {}  {}",
                    crate::ui::paint(color, "\x1b[1;36m", "›"),
                    crate::ui::paint(color, "\x1b[1m", &label),
                    dim(&detail)
                )
            } else {
                format!("  {label}  {}", dim(&detail))
            });
        }
        if let Some(t) = self.typed().filter(|_| shown.is_empty()) {
            out.push(dim(&format!("  nothing listed matches; Enter uses {t}")));
        } else if shown.is_empty() {
            out.push(dim("  nothing matches"));
        } else if shown.len() > ROWS {
            out.push(dim(&format!("  {} of {}", self.selected + 1, shown.len())));
        }
        out
    }
}

/// Cuts `s` to `width` columns, ending in … when anything was cut.
fn fit(s: &str, width: usize) -> String {
    if crate::term::text_width(s) <= width {
        return s.to_string();
    }
    let mut used = 0;
    let mut out: String = s
        .chars()
        .take_while(|c| {
            used += crate::term::char_width(*c);
            used < width
        })
        .collect();
    out.push('…');
    out
}

/// Draws `lines` over the ones drawn last time, whose first row is `up`
/// rows above the cursor, and puts the cursor at `(row, col)` of the new
/// lines. Returns that row, the next call's `up`.
fn draw(lines: &[String], up: usize, (row, col): (usize, usize)) -> usize {
    let mut s = String::from("\r");
    if up > 0 {
        s.push_str(&format!("\x1b[{up}A"));
    }
    s.push_str("\x1b[J");
    s.push_str(&lines.join("\n"));
    let back = lines.len().saturating_sub(1).saturating_sub(row);
    if back > 0 {
        s.push_str(&format!("\x1b[{back}A"));
    }
    s.push('\r');
    if col > 0 {
        s.push_str(&format!("\x1b[{col}C"));
    }
    print!("{s}");
    let _ = std::io::stdout().flush();
    row
}

fn clear(up: usize) {
    draw(&[], up, (0, 0));
}

/// Runs a picker until a choice is made or it is cancelled, then removes it
/// from the screen.
pub fn pick(keys: &mut Keys, picker: Picker, color: bool) -> Option<usize> {
    match choose(keys, picker, color) {
        Step::Picked(i) => Some(i),
        _ => None,
    }
}

/// Like `pick`, also giving back text typed into a picker that allows it.
pub fn choose(keys: &mut Keys, mut picker: Picker, color: bool) -> Step {
    // The cursor waits on the filter line, after what has been typed.
    let show = |p: &Picker, up| {
        let at = (1, 2 + crate::term::text_width(&p.query));
        draw(&p.lines(crate::term::width(), color), up, at)
    };
    let mut rows = show(&picker, 0);
    loop {
        let step = match keys.next() {
            Some(k) => picker.key(k),
            None => Step::Cancelled,
        };
        match step {
            Step::Pending => rows = show(&picker, rows),
            done => {
                clear(rows);
                return done;
            }
        }
    }
}

/// Asks for one line. A hidden answer shows a dot per character. Esc or an
/// empty answer gives `None`.
pub fn ask(keys: &mut Keys, question: &str, hidden: bool) -> Option<String> {
    let mut answer = String::new();
    let show = |a: &str| {
        let shown = if hidden {
            "•".repeat(a.chars().count().min(40))
        } else {
            a.to_string()
        };
        format!("{question}{shown}")
    };
    let line = |a: &str| {
        let text = show(a);
        let col = crate::term::text_width(&text);
        (text, col)
    };
    let (text, col) = line(&answer);
    let mut rows = draw(&[text], 0, (0, col));
    loop {
        match keys.next() {
            Some(Key::Enter) => break,
            Some(Key::Esc | Key::Ctrl('c')) | None => {
                answer.clear();
                break;
            }
            Some(Key::Char(c)) => answer.push(c),
            Some(Key::Paste(t)) => answer.push_str(t.trim()),
            Some(Key::Backspace) => {
                answer.pop();
            }
            Some(Key::Ctrl('u')) => answer.clear(),
            _ => {}
        }
        let (text, col) = line(&answer);
        rows = draw(&[text], rows, (0, col));
    }
    clear(rows);
    let answer = answer.trim().to_string();
    (!answer.is_empty()).then_some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Picker {
        Picker::new(
            "Model",
            vec![
                Choice::new("anthropic/claude-opus-5-5", "Anthropic"),
                Choice::new("anthropic/claude-haiku-4-5", "Anthropic"),
                Choice::new("openai/gpt-5", "OpenAI"),
            ],
            Some(1),
        )
    }

    #[test]
    fn enter_keeps_the_current_choice() {
        assert_eq!(models().key(Key::Enter), Step::Picked(1));
    }

    #[test]
    fn typing_filters_by_every_word() {
        let mut p = models();
        for c in "anth opus".chars() {
            p.key(Key::Char(c));
        }
        assert_eq!(p.filtered(), vec![0]);
        assert_eq!(p.key(Key::Enter), Step::Picked(0));
        p.key(Key::Ctrl('u'));
        p.key(Key::Paste("openai".into()));
        assert_eq!(p.key(Key::Enter), Step::Picked(2));
        p.key(Key::Char('z'));
        assert_eq!(p.key(Key::Enter), Step::Pending, "nothing to pick");
        assert!(
            p.lines(80, false)
                .last()
                .unwrap()
                .contains("nothing matches")
        );
    }

    #[test]
    fn typed_text_is_used_only_when_allowed_and_unmatched() {
        let mut p = models().allow_typed("local/");
        p.key(Key::Paste("qwen3".into()));
        assert!(
            p.lines(80, false)
                .last()
                .unwrap()
                .ends_with("Enter uses local/qwen3")
        );
        assert_eq!(p.key(Key::Enter), Step::Typed("local/qwen3".into()));
        p.key(Key::Ctrl('u'));
        p.key(Key::Paste("other/qwen3".into()));
        assert_eq!(p.key(Key::Enter), Step::Typed("other/qwen3".into()));
        let mut p = models().allow_typed("local/");
        p.key(Key::Paste("gpt".into()));
        assert_eq!(p.key(Key::Enter), Step::Picked(2), "a match wins");
        let mut p = models();
        p.key(Key::Paste("qwen3".into()));
        assert_eq!(p.key(Key::Enter), Step::Pending);
    }

    #[test]
    fn arrows_move_and_stop_at_the_ends() {
        let mut p = models();
        p.key(Key::Down);
        p.key(Key::Down);
        p.key(Key::Down);
        assert_eq!(p.key(Key::Enter), Step::Picked(2));
        for _ in 0..5 {
            p.key(Key::Up);
        }
        assert_eq!(p.key(Key::Enter), Step::Picked(0));
        assert_eq!(p.key(Key::Esc), Step::Cancelled);
    }

    #[test]
    fn long_lists_scroll_and_rows_fit_the_width() {
        let many = (0..30)
            .map(|i| {
                Choice::new(
                    format!("model-{i:02}"),
                    "a detail that is rather long indeed",
                )
            })
            .collect();
        let mut p = Picker::new("Model", many, Some(25));
        let lines = p.lines(30, false);
        assert_eq!(lines.len(), 2 + ROWS + 1);
        assert!(lines.iter().any(|l| l.starts_with("› model-25")));
        assert_eq!(lines.last().unwrap(), "  26 of 30");
        assert!(
            lines[2..].iter().all(|l| crate::term::text_width(l) <= 30),
            "{lines:?}"
        );
        p.key(Key::Char('0'));
        assert_eq!(p.key(Key::Enter), Step::Picked(0));
    }
}
