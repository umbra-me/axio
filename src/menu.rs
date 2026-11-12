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
    /// Tab in a picker with an effort row: use the choice for this session only.
    Session(usize),
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
    /// Effort levels each choice takes, and the level being shown.
    effort: Option<(Vec<Vec<String>>, String)>,
}

/// Whether a choice becomes the default or lasts for this session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Default,
    Session,
}

const ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// `level` if `levels` has it, else the nearest level below it, else the
/// lowest one.
pub fn clamp(level: &str, levels: &[String]) -> Option<String> {
    let rank = |l: &str| ORDER.iter().position(|o| *o == l).unwrap_or(0);
    levels
        .iter()
        .filter(|l| rank(l) <= rank(level))
        .max_by_key(|l| rank(l))
        .or(levels.first())
        .cloned()
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
            effort: None,
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

    /// Adds an effort row that Left and Right adjust for the highlighted
    /// choice, `levels[i]` being what choice `i` takes, and makes Enter the
    /// default and Tab this session only.
    pub fn with_effort(mut self, levels: Vec<Vec<String>>, current: Option<&str>) -> Picker {
        let start = current.unwrap_or("high").to_string();
        self.effort = Some((levels, start));
        self
    }

    fn levels(&self) -> &[String] {
        let chosen = self.filtered().get(self.selected).copied();
        match (&self.effort, chosen) {
            (Some((levels, _)), Some(i)) => levels.get(i).map_or(&[], Vec::as_slice),
            _ => &[],
        }
    }

    /// The effort shown for the highlighted choice; `None` when it takes none.
    pub fn effort(&self) -> Option<String> {
        let (_, level) = self.effort.as_ref()?;
        clamp(level, self.levels())
    }

    fn step_effort(&mut self, up: bool) {
        let levels = self.levels().to_vec();
        let Some(now) = self.effort() else { return };
        let at = levels.iter().position(|l| *l == now).unwrap_or(0);
        let next = if up {
            (at + 1).min(levels.len() - 1)
        } else {
            at.saturating_sub(1)
        };
        if let Some((_, level)) = &mut self.effort {
            *level = levels[next].clone();
        }
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
            Key::Ctrl('i') if self.effort.is_some() => {
                if let Some(&i) = shown.get(self.selected) {
                    return Step::Session(i);
                }
            }
            Key::Left if self.effort.is_some() => self.step_effort(false),
            Key::Right if self.effort.is_some() => self.step_effort(true),
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
        let hint = if self.effort.is_some() {
            "type to filter · ↑↓ move"
        } else {
            "type to filter · ↑↓ move · Enter choose · Esc cancel"
        };
        let mut out = vec![
            format!("{}  {}", self.title, dim(hint)),
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
        if self.effort.is_some() {
            out.push(String::new());
            out.push(match self.effort() {
                Some(level) => format!(
                    "{} {} {}",
                    crate::ui::paint(color, "\x1b[33m", "◉"),
                    crate::ui::paint(color, "\x1b[1m", &format!("{level} effort")),
                    dim("←/→ to adjust")
                ),
                None => dim("  this model has no effort setting"),
            });
            out.push(String::new());
            out.push(dim(
                "Enter sets as default · Tab this session only · Esc cancel",
            ));
        }
        out
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Slide {
    Pending,
    Chosen(usize, Scope),
    Cancelled,
}

/// A row of levels from faster to smarter, moved with Left and Right.
pub struct Slider {
    title: String,
    levels: Vec<String>,
    /// What each level does, when the provider says.
    notes: Vec<String>,
    at: usize,
}

impl Slider {
    pub fn new(title: &str, levels: Vec<String>, notes: Vec<String>, current: &str) -> Slider {
        let at = clamp(current, &levels)
            .and_then(|c| levels.iter().position(|l| *l == c))
            .unwrap_or(0);
        Slider {
            title: title.into(),
            levels,
            notes,
            at,
        }
    }

    pub fn key(&mut self, key: Key) -> Slide {
        match key {
            Key::Left | Key::Char('h') => self.at = self.at.saturating_sub(1),
            Key::Right | Key::Char('l') => {
                self.at = (self.at + 1).min(self.levels.len().saturating_sub(1));
            }
            Key::Enter => return Slide::Chosen(self.at, Scope::Default),
            Key::Ctrl('i') | Key::Char('s') => return Slide::Chosen(self.at, Scope::Session),
            Key::Esc | Key::Ctrl('c') | Key::Ctrl('g') => return Slide::Cancelled,
            _ => {}
        }
        Slide::Pending
    }

    pub fn lines(&self, width: usize, color: bool) -> Vec<String> {
        let dim = |s: &str| crate::ui::paint(color, crate::ui::DIM, s);
        let n = self.levels.len().max(1);
        let cell = self
            .levels
            .iter()
            .map(|l| l.chars().count() + 4)
            .max()
            .unwrap_or(8)
            .min(width.saturating_sub(4) / n)
            .max(4);
        let span = cell * n;
        let center = |i: usize| i * cell + cell / 2;
        let mut names = String::new();
        // Colour codes take no columns, so the width drawn is counted apart.
        let mut used = 0;
        for (i, l) in self.levels.iter().enumerate() {
            let len = l.chars().count();
            let start = center(i).saturating_sub(len / 2);
            let pad = start.saturating_sub(used);
            names.push_str(&" ".repeat(pad));
            used += pad + len;
            names.push_str(&if i == self.at {
                crate::ui::paint(color, "\x1b[1;35m", l)
            } else {
                dim(l)
            });
        }
        let mark = center(self.at).min(span.saturating_sub(1));
        let rule = format!(
            "{}{}{}",
            "─".repeat(mark),
            crate::ui::paint(color, "\x1b[1m", "▲"),
            "─".repeat(span.saturating_sub(mark + 1))
        );
        let ends = format!("Faster{}Smarter", " ".repeat(span.saturating_sub(13)));
        let mut out = vec![
            crate::ui::paint(color, "\x1b[1m", &self.title),
            String::new(),
            format!("  {ends}"),
            format!("  {rule}"),
            format!("  {names}"),
        ];
        if let Some(note) = self.notes.get(self.at).filter(|n| !n.is_empty()) {
            out.push(String::new());
            out.push(format!("  {}", dim(note)));
        }
        out.push(String::new());
        out.push(dim(
            "←/→ to adjust · Enter sets as default · Tab this session only · Esc cancel",
        ));
        out
    }
}

/// Runs a slider until a level is chosen or it is cancelled.
pub fn slide(keys: &mut Keys, mut slider: Slider, color: bool) -> Option<(usize, Scope)> {
    let show = |s: &Slider, up| {
        let lines = s.lines(crate::term::width(), color);
        let last = lines.len().saturating_sub(1);
        draw(&lines, up, (last, 0))
    };
    let mut rows = show(&slider, 0);
    loop {
        let step = match keys.next() {
            Some(k) => slider.key(k),
            None => Slide::Cancelled,
        };
        match step {
            Slide::Pending => rows = show(&slider, rows),
            Slide::Chosen(i, scope) => {
                clear(rows);
                return Some((i, scope));
            }
            Slide::Cancelled => {
                clear(rows);
                return None;
            }
        }
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
pub fn pick(keys: &mut Keys, mut picker: Picker, color: bool) -> Option<usize> {
    match choose(keys, &mut picker, color) {
        Step::Picked(i) => Some(i),
        _ => None,
    }
}

/// Like `pick`, also giving back text typed into a picker that allows it.
pub fn choose(keys: &mut Keys, picker: &mut Picker, color: bool) -> Step {
    // The cursor waits on the filter line, after what has been typed.
    let show = |p: &Picker, up| {
        let at = (1, 2 + crate::term::text_width(&p.query));
        draw(&p.lines(crate::term::width(), color), up, at)
    };
    let mut rows = show(picker, 0);
    loop {
        let step = match keys.next() {
            Some(k) => picker.key(k),
            None => Step::Cancelled,
        };
        match step {
            Step::Pending => rows = show(picker, rows),
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

    fn levels(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_model_picker_adjusts_effort_for_the_highlighted_model() {
        let all = levels(&["low", "medium", "high", "xhigh", "max"]);
        let mut p = models().with_effort(
            vec![all.clone(), Vec::new(), levels(&["low", "medium", "high"])],
            Some("xhigh"),
        );
        let text = p.lines(80, false).join("\n");
        assert!(text.contains("no effort setting"), "{text}");
        assert!(text.contains("Enter sets as default") && text.contains("Tab this session only"));
        p.key(Key::Up);
        assert_eq!(p.effort().as_deref(), Some("xhigh"));
        p.key(Key::Right);
        p.key(Key::Right);
        assert_eq!(p.effort().as_deref(), Some("max"), "stops at the top");
        assert!(p.lines(80, false).join("\n").contains("max effort"));
        p.key(Key::Left);
        p.key(Key::Down);
        p.key(Key::Down);
        assert_eq!(
            p.effort().as_deref(),
            Some("high"),
            "clamped to what the model takes"
        );
        assert_eq!(p.key(Key::Ctrl('i')), Step::Session(2));
        assert_eq!(p.key(Key::Enter), Step::Picked(2));
    }

    #[test]
    fn the_effort_slider_moves_between_levels() {
        let mut s = Slider::new(
            "Effort",
            levels(&["low", "medium", "high", "xhigh", "max"]),
            Vec::new(),
            "high",
        );
        let lines = s.lines(80, false);
        let text = lines.join("\n");
        assert!(text.contains("Faster") && text.contains("Smarter"));
        let marker = lines.iter().find(|l| l.contains('▲')).unwrap();
        let names = lines.iter().find(|l| l.contains("medium")).unwrap();
        let at = |l: &str, s: &str| l[..l.find(s).unwrap()].chars().count();
        let high = at(names, "high");
        assert!(
            (high..high + 4).contains(&at(marker, "▲")),
            "marker under high:\n{text}"
        );
        s.key(Key::Right);
        s.key(Key::Right);
        s.key(Key::Right);
        assert_eq!(s.key(Key::Enter), Slide::Chosen(4, Scope::Default));
        s.key(Key::Left);
        assert_eq!(s.key(Key::Ctrl('i')), Slide::Chosen(3, Scope::Session));
        assert_eq!(s.key(Key::Esc), Slide::Cancelled);
    }

    #[test]
    fn colour_does_not_shift_the_slider_labels() {
        let s = Slider::new(
            "Effort",
            levels(&["low", "medium", "high"]),
            Vec::new(),
            "medium",
        );
        let plain = s.lines(80, false);
        let strip = |l: &str| {
            let mut out = String::new();
            let mut esc = false;
            for c in l.chars() {
                match (esc, c) {
                    (false, '\x1b') => esc = true,
                    (true, 'm') => esc = false,
                    (false, c) => out.push(c),
                    _ => {}
                }
            }
            out
        };
        let coloured: Vec<String> = s.lines(80, true).iter().map(|l| strip(l)).collect();
        assert_eq!(coloured, plain);
    }

    #[test]
    fn the_slider_shows_the_level_description_when_known() {
        let s = Slider::new(
            "Effort",
            levels(&["low", "high"]),
            levels(&["Fast responses", "Deeper reasoning"]),
            "high",
        );
        assert!(s.lines(80, false).join("\n").contains("Deeper reasoning"));
    }
}
