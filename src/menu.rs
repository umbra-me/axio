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
    /// Ctrl-S in a picker with an effort row: use the choice for this session only.
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

    /// The highlighted choice.
    pub fn current(&self) -> Option<usize> {
        self.filtered().get(self.selected).copied()
    }

    fn select(&mut self, choice: usize) {
        if let Some(at) = self.filtered().iter().position(|&i| i == choice) {
            self.selected = at;
            self.scroll();
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
            Key::Ctrl('s') if self.effort.is_some() => {
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
        let dim = |s: &str| crate::ui::paint(color, crate::ui::DIM, s);
        let hint = if self.effort.is_some() {
            "type to filter · ↑↓ move"
        } else {
            "type to filter · ↑↓ move · Enter choose · Esc cancel"
        };
        let mut out = vec![format!("{}  {}", self.title, dim(hint))];
        out.extend(self.body(width, color));
        if self.effort.is_some() {
            out.push(String::new());
            out.push(self.effort_line(color));
            out.push(String::new());
            out.push(dim(&fit(
                "Enter sets as default · Ctrl-S this session only · Esc cancel",
                width,
            )));
        }
        out
    }

    /// The filter line, the visible choices and a status line.
    fn body(&self, width: usize, color: bool) -> Vec<String> {
        let shown = self.filtered();
        let dim = |s: &str| crate::ui::paint(color, crate::ui::DIM, s);
        let mut out = vec![format!("› {}", fit(&self.query, width.saturating_sub(2)))];
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
        let status = if let Some(t) = self.typed().filter(|_| shown.is_empty()) {
            format!("  nothing listed matches; Enter uses {t}")
        } else if shown.is_empty() {
            "  nothing matches".to_string()
        } else if shown.len() > ROWS {
            format!("  {} of {}", self.selected + 1, shown.len())
        } else {
            String::new()
        };
        if !status.is_empty() {
            out.push(dim(&fit(&status, width)));
        }
        out
    }

    fn effort_line(&self, color: bool) -> String {
        let dim = |s: &str| crate::ui::paint(color, crate::ui::DIM, s);
        match self.effort() {
            Some(level) => format!(
                "{} {} {}",
                crate::ui::paint(color, "\x1b[33m", "◉"),
                crate::ui::paint(color, "\x1b[1m", &format!("{level} effort")),
                dim("←/→ to adjust")
            ),
            None => dim("  this model has no effort setting"),
        }
    }
}

/// One provider in the model browser.
pub struct Group {
    pub name: String,
    pub signed_in: bool,
    /// Its models; filtered and moved through when the models side has focus.
    pub picker: Picker,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Browse {
    Pending,
    /// A provider's model, by group and choice.
    Picked(usize, usize, Scope),
    /// A model typed into a provider's list that the list does not hold.
    Typed(usize, String, Scope),
    /// A provider that is not set up yet.
    SignIn(usize),
    Cancelled,
}

/// Providers on the left, the highlighted provider's models on the right.
/// Tab moves between the two sides.
pub struct Browser {
    groups: Vec<Group>,
    at: usize,
    /// Whether the models side has focus.
    models: bool,
}

/// Below this width only the side with focus is drawn.
const TWO_SIDES: usize = 60;

/// The columns `s` takes on screen, leaving out colour codes.
fn shown_width(s: &str) -> usize {
    let mut plain = String::new();
    let mut escape = false;
    for c in s.chars() {
        match (escape, c) {
            (false, '\x1b') => escape = true,
            (true, 'm') => escape = false,
            (false, c) => plain.push(c),
            _ => {}
        }
    }
    crate::term::text_width(&plain)
}

impl Browser {
    /// Opens on group `at`, on its choice `current` when given.
    pub fn new(mut groups: Vec<Group>, at: usize, current: Option<usize>) -> Browser {
        let at = at.min(groups.len().saturating_sub(1));
        if let (Some(g), Some(c)) = (groups.get_mut(at), current) {
            g.picker.select(c);
        }
        let models = groups.get(at).is_some_and(|g| g.signed_in);
        Browser { groups, at, models }
    }

    pub fn picker(&self) -> &Picker {
        &self.groups[self.at].picker
    }

    pub fn key(&mut self, key: Key) -> Browse {
        if matches!(key, Key::Esc | Key::Ctrl('c') | Key::Ctrl('g')) {
            return Browse::Cancelled;
        }
        let at = self.at;
        if self.models {
            let picker = &mut self.groups[at].picker;
            return match key {
                Key::Ctrl('i') => {
                    self.models = false;
                    Browse::Pending
                }
                Key::Ctrl('s') => picker
                    .current()
                    .map_or(Browse::Pending, |i| Browse::Picked(at, i, Scope::Session)),
                k => match picker.key(k) {
                    Step::Picked(i) => Browse::Picked(at, i, Scope::Default),
                    Step::Session(i) => Browse::Picked(at, i, Scope::Session),
                    Step::Typed(t) => Browse::Typed(at, t, Scope::Default),
                    Step::Cancelled => Browse::Cancelled,
                    Step::Pending => Browse::Pending,
                },
            };
        }
        let signed_in = self.groups[at].signed_in;
        match key {
            Key::Up | Key::Ctrl('p') => self.at = at.saturating_sub(1),
            Key::Down | Key::Ctrl('n') => self.at = (at + 1).min(self.groups.len() - 1),
            Key::Enter | Key::Right | Key::Ctrl('i') if !signed_in => return Browse::SignIn(at),
            Key::Enter | Key::Right | Key::Ctrl('i') => self.models = true,
            // Typing searches the highlighted provider's models.
            k @ (Key::Char(_) | Key::Paste(_)) if signed_in => {
                self.models = true;
                self.groups[at].picker.key(k);
            }
            _ => {}
        }
        Browse::Pending
    }

    fn provider_rows(&self, width: usize, color: bool) -> Vec<String> {
        let dim = |s: &str| crate::ui::paint(color, crate::ui::DIM, s);
        let mut out = vec![crate::ui::paint(color, "\x1b[1m", "Providers")];
        for (i, g) in self.groups.iter().enumerate() {
            let count = if g.signed_in {
                g.picker.choices.len().to_string()
            } else {
                "sign in".into()
            };
            let name = fit(&g.name, width.saturating_sub(4 + count.chars().count()));
            let gap = width.saturating_sub(2 + name.chars().count() + count.chars().count());
            let row = format!("{name}{}{}", " ".repeat(gap), dim(&count));
            out.push(match (i == self.at, self.models) {
                (true, false) => format!(
                    "{} {}",
                    crate::ui::paint(color, "\x1b[1;36m", "›"),
                    crate::ui::paint(color, "\x1b[1m", &row)
                ),
                (true, true) => format!("{} {}", crate::ui::paint(color, "\x1b[36m", "•"), row),
                _ if g.signed_in => format!("  {row}"),
                _ => format!("  {}", dim(&row)),
            });
        }
        out
    }

    fn model_rows(&self, width: usize, color: bool) -> Vec<String> {
        let g = &self.groups[self.at];
        let dim = |s: &str| crate::ui::paint(color, crate::ui::DIM, s);
        let head = if g.signed_in {
            format!("{} · {} models", g.name, g.picker.choices.len())
        } else {
            g.name.clone()
        };
        let mut out = vec![crate::ui::paint(color, "\x1b[1m", &fit(&head, width))];
        if g.signed_in {
            out.extend(g.picker.body(width, color));
        } else {
            out.push(String::new());
            out.push(dim(&fit("  Not set up. Enter to sign in.", width)));
        }
        out
    }

    pub fn lines(&self, width: usize, color: bool) -> Vec<String> {
        let dim = |s: &str| crate::ui::paint(color, crate::ui::DIM, s);
        let mut out = vec![crate::ui::paint(color, "\x1b[1m", "Models")];
        if width < TWO_SIDES {
            if self.models {
                out.extend(self.model_rows(width, color));
            } else {
                out.extend(self.provider_rows(width, color));
            }
        } else {
            let left_width = self.left_width(width);
            let left = self.provider_rows(left_width, color);
            let right = self.model_rows(width.saturating_sub(left_width + 3), color);
            for row in 0..left.len().max(right.len()) {
                let l = left.get(row).map_or("", String::as_str);
                let pad = left_width.saturating_sub(shown_width(l));
                let r = right.get(row).map_or("", String::as_str);
                out.push(format!("{l}{} {} {r}", " ".repeat(pad), dim("│")));
            }
        }
        out.push(String::new());
        if self.groups[self.at].signed_in && self.picker().effort.is_some() {
            out.push(self.picker().effort_line(color));
        }
        let keys = if self.models {
            "Tab providers · Enter sets as default · Ctrl-S this session only · Esc cancel"
        } else {
            "↑↓ provider · Enter or → models · type to search · Esc cancel"
        };
        out.push(dim(&fit(keys, width)));
        out
    }

    fn left_width(&self, width: usize) -> usize {
        self.groups
            .iter()
            .map(|g| g.name.chars().count() + 12)
            .max()
            .unwrap_or(20)
            .clamp(18, 30)
            .min(width / 3)
    }

    /// Where the cursor waits: on the filter line, or the highlighted provider.
    fn cursor(&self, width: usize) -> (usize, usize) {
        if !self.models {
            return (2 + self.at, 0);
        }
        let left = if width < TWO_SIDES {
            0
        } else {
            self.left_width(width) + 3
        };
        (2, left + 2 + crate::term::text_width(&self.picker().query))
    }
}

/// Runs the browser until a model is chosen, a provider needs signing in,
/// or it is cancelled.
pub fn browse(keys: &mut Keys, browser: &mut Browser, color: bool) -> Browse {
    let show = |b: &Browser, up| {
        let width = crate::term::width();
        draw(&b.lines(width, color), up, b.cursor(width))
    };
    let mut rows = show(browser, 0);
    loop {
        let step = match keys.next() {
            Some(k) => browser.key(k),
            None => Browse::Cancelled,
        };
        if step == Browse::Pending {
            rows = show(browser, rows);
        } else {
            clear(rows);
            return step;
        }
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
            Key::Char('s') | Key::Ctrl('s') | Key::Ctrl('i') => {
                return Slide::Chosen(self.at, Scope::Session);
            }
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
            "←/→ to adjust · Enter sets as default · s this session only · Esc cancel",
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
        assert!(
            text.contains("Enter sets as default") && text.contains("Ctrl-S this session only")
        );
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
        assert_eq!(p.key(Key::Ctrl('s')), Step::Session(2));
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
        assert_eq!(s.key(Key::Char('s')), Slide::Chosen(3, Scope::Session));
        assert_eq!(s.key(Key::Ctrl('s')), Slide::Chosen(3, Scope::Session));
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

    fn browser() -> Browser {
        let group = |name: &str, signed: bool, models: &[&str]| Group {
            name: name.into(),
            signed_in: signed,
            picker: Picker::new(
                "",
                models.iter().map(|m| Choice::new(*m, "")).collect(),
                None,
            ),
        };
        Browser::new(
            vec![
                group("anthropic", true, &["claude-haiku-4-5", "claude-opus-5-5"]),
                group("chatgpt", true, &["gpt-6-luna", "gpt-6-sol"]),
                group("openai", false, &[]),
            ],
            1,
            Some(1),
        )
    }

    #[test]
    fn the_browser_opens_on_the_current_model() {
        let mut b = browser();
        assert_eq!(b.key(Key::Enter), Browse::Picked(1, 1, Scope::Default));
        assert_eq!(b.key(Key::Ctrl('s')), Browse::Picked(1, 1, Scope::Session));
    }

    #[test]
    fn tab_cycles_between_providers_and_their_models() {
        let mut b = browser();
        b.key(Key::Ctrl('i'));
        b.key(Key::Up);
        let text = b.lines(100, false).join("\n");
        assert!(
            text.contains("claude-opus-5-5"),
            "previews the provider's models:\n{text}"
        );
        assert!(!text.contains("gpt-6-sol"));
        b.key(Key::Enter);
        b.key(Key::Down);
        assert_eq!(b.key(Key::Enter), Browse::Picked(0, 1, Scope::Default));
        b.key(Key::Ctrl('i'));
        b.key(Key::Right);
        assert_eq!(
            b.key(Key::Enter),
            Browse::Picked(0, 1, Scope::Default),
            "→ enters models too"
        );
    }

    #[test]
    fn typing_on_the_providers_side_searches_that_provider() {
        let mut b = browser();
        b.key(Key::Ctrl('i'));
        b.key(Key::Char('s'));
        b.key(Key::Char('o'));
        b.key(Key::Char('l'));
        assert_eq!(b.key(Key::Enter), Browse::Picked(1, 1, Scope::Default));
    }

    #[test]
    fn a_provider_not_set_up_offers_sign_in() {
        let mut b = browser();
        b.key(Key::Ctrl('i'));
        b.key(Key::Down);
        let text = b.lines(100, false).join("\n");
        assert!(text.contains("Enter to sign in"), "{text}");
        assert_eq!(b.key(Key::Enter), Browse::SignIn(2));
        assert_eq!(b.key(Key::Esc), Browse::Cancelled);
    }

    #[test]
    fn rows_fit_and_a_narrow_terminal_shows_one_side() {
        let b = browser();
        for width in [100, 50] {
            for l in b.lines(width, false) {
                assert!(crate::term::text_width(&l) <= width, "{width}: {l:?}");
            }
        }
        let narrow = b.lines(50, false).join("\n");
        assert!(narrow.contains("gpt-6-sol") && !narrow.contains("anthropic   "));
    }
}
