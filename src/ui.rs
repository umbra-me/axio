//! The interactive terminal: streamed markdown on stdout, and on stderr a
//! spinner with elapsed time while the model or a tool works, a short header
//! for each tool call and a compact summary of its result.

use crate::agent::Ui;
use crate::markdown::Markdown;
use crate::provider::{Delta, Stop, Usage};
use crate::term;
use serde_json::Value;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

pub struct Term {
    /// Colour and spinner on stderr.
    pub color: bool,
    pub keys: Option<term::Keys>,
    pub turn: Usage,
    pub turn_started: Instant,
    /// Totals for the whole session, for the summary on exit.
    pub turns: u32,
    pub total_out: u64,
    started: Instant,
    cwd: PathBuf,
    md: Markdown,
    spinner: Option<(String, Instant)>,
    drawn: bool,
    last_frame: Instant,
    frame: usize,
    tool: Option<(String, Value)>,
    /// Summary of the model's current thinking, shown after the spinner.
    thought: String,
    /// The tool call the model is writing now.
    writing: Option<Writing>,
    /// What the turn printed last, to put a blank line between text and tools.
    last: Shown,
}

#[derive(PartialEq, Clone, Copy)]
enum Shown {
    Nothing,
    Text,
    Tool,
}

impl Term {
    pub fn new(cwd: &Path, color_err: bool, color_out: bool) -> Term {
        Term {
            color: color_err,
            keys: None,
            turn: Usage::default(),
            turn_started: Instant::now(),
            turns: 0,
            total_out: 0,
            started: Instant::now(),
            cwd: cwd.to_path_buf(),
            md: Markdown::new(color_out),
            spinner: None,
            drawn: false,
            last_frame: Instant::now(),
            frame: 0,
            tool: None,
            writing: None,
            thought: String::new(),
            last: Shown::Nothing,
        }
    }

    pub fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("{code}{s}{RESET}")
        } else {
            s.to_string()
        }
    }

    pub fn dim(&self, s: &str) -> String {
        self.paint(DIM, s)
    }

    pub fn begin_turn(&mut self) {
        self.turn = Usage::default();
        self.turn_started = Instant::now();
        self.turns += 1;
        self.md.reset();
        self.last = Shown::Nothing;
    }

    /// Changes the label without restarting the clock.
    fn relabel(&mut self, label: String) {
        match &mut self.spinner {
            Some((l, _)) if *l != label => {
                *l = label;
                self.tick(false);
            }
            Some(_) => {}
            None => self.spin(&label),
        }
    }

    fn spin(&mut self, label: &str) {
        if self.spinner.as_ref().is_none_or(|(l, _)| l != label) {
            self.spinner = Some((label.to_string(), Instant::now()));
            self.thought.clear();
        }
        self.tick(true);
    }

    fn tick(&mut self, now: bool) {
        if !self.color {
            return;
        }
        let Some((label, since)) = &self.spinner else {
            return;
        };
        if !now && self.last_frame.elapsed() < Duration::from_millis(80) {
            return;
        }
        self.frame = (self.frame + 1) % FRAMES.len();
        let secs = since.elapsed().as_secs();
        let time = if secs > 0 {
            format!(" · {secs}s")
        } else {
            String::new()
        };
        let width = term_width().saturating_sub(4);
        let mut text: String = format!("{label}{time}").chars().take(width).collect();
        let room = width.saturating_sub(text.chars().count() + 3);
        if let Some(note) = latest_line(&self.thought, room) {
            text.push_str(&format!(" · {note}"));
        }
        eprint!(
            "\r\x1b[K{CYAN}{}{RESET} {DIM}{text}{RESET}",
            FRAMES[self.frame]
        );
        let _ = io::stderr().flush();
        self.drawn = true;
        self.last_frame = Instant::now();
    }

    fn unspin(&mut self) {
        self.spinner = None;
        if self.drawn {
            eprint!("\r\x1b[K");
            let _ = io::stderr().flush();
            self.drawn = false;
        }
    }

    /// Finishes any half-written line of model text.
    fn shown(&mut self, now: Shown) {
        if self.last != Shown::Nothing && self.last != now {
            eprintln!();
        }
        self.last = now;
    }

    fn settle(&mut self) {
        self.unspin();
        let rest = self.md.flush();
        if !rest.is_empty() {
            print!("{rest}");
            let _ = io::stdout().flush();
        }
    }

    /// One dim line on exit: what the session did and how to come back.
    pub fn farewell(&self, session: Option<&Path>) -> String {
        let secs = self.started.elapsed().as_secs();
        let time = match secs {
            0..60 => format!("{secs}s"),
            60..3600 => format!("{}m {}s", secs / 60, secs % 60),
            _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
        };
        let turns = match self.turns {
            1 => "1 turn".to_string(),
            n => format!("{n} turns"),
        };
        let mut s = format!("{turns} · {} out · {time}", tokens(self.total_out));
        if session.is_some() && self.turns > 0 {
            s.push_str(" · `axio -c` continues this session");
        }
        self.dim(&s)
    }

    /// The dim line after a turn: context, output, cache and time.
    pub fn status(&self, context: u64, window: u64) -> String {
        let t = &self.turn;
        let sent = t.input + t.cache_read + t.cache_write;
        let cached = (t.cache_read * 100)
            .checked_div(sent)
            .filter(|p| *p > 0)
            .map(|p| format!(" · {p}% cached"))
            .unwrap_or_default();
        self.dim(&format!(
            "{} of {} context · {} out{cached} · {:.1}s",
            tokens(context),
            tokens(window),
            tokens(t.output),
            self.turn_started.elapsed().as_secs_f64()
        ))
    }
}

/// Progress on a tool call whose input is still streaming.
struct Writing {
    name: String,
    json: String,
    lines: usize,
    escaped: bool,
}

impl Writing {
    fn new(name: &str) -> Writing {
        Writing {
            name: name.to_string(),
            json: String::new(),
            lines: 0,
            escaped: false,
        }
    }

    fn push(&mut self, part: &str) {
        for c in part.chars() {
            if self.escaped {
                self.lines += usize::from(c == 'n');
                self.escaped = false;
            } else {
                self.escaped = c == '\\';
            }
        }
        self.json.push_str(part);
    }

    /// "Write src/main.rs · 212 lines", from what has arrived so far.
    fn label(&self, cwd: &Path) -> String {
        let mut input = serde_json::Map::new();
        for key in ["file_path", "command", "pattern", "url"] {
            if let Some(v) = string_field(&self.json, key) {
                input.insert(key.into(), Value::String(v));
            }
        }
        let (title, detail) = tool_title(&self.name, &Value::Object(input), cwd);
        let mut s = format!("writing {title}");
        if !detail.is_empty() {
            s.push_str(&format!(" {detail}"));
        }
        if self.lines > 0 {
            s.push_str(&format!(" · {} lines", self.lines + 1));
        } else if self.json.len() >= 1024 {
            s.push_str(&format!(" · {:.1} KB", self.json.len() as f64 / 1024.0));
        }
        s
    }
}

/// A complete string field from partial JSON, if it has fully arrived.
fn string_field(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut end = None;
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        match (escaped, c) {
            (true, _) => escaped = false,
            (false, '\\') => escaped = true,
            (false, '"') => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    serde_json::from_str(&format!("\"{}\"", &rest[..end?])).ok()
}

/// The newest line of `text`, without markdown emphasis, fitted to `room`
/// characters by keeping its end.
fn latest_line(text: &str, room: usize) -> Option<String> {
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_start_matches('#').replace("**", "");
    let line = line.trim();
    let n = line.chars().count();
    if room < 12 || line.is_empty() {
        return None;
    }
    Some(if n <= room {
        line.to_string()
    } else {
        let tail: String = line.chars().skip(n - (room - 1)).collect();
        format!("…{tail}")
    })
}

/// Where a resumed session left off: the last prompt and the end of the
/// last reply, as dim lines.
pub fn recap(messages: &[crate::message::Message]) -> Vec<String> {
    use crate::message::{Block, Message};
    let prompt = messages.iter().rev().find_map(|m| match m {
        Message::User { content } => content.iter().find_map(|b| match b {
            Block::Text { text }
                if !text.starts_with('[') && !text.starts_with(crate::compact::SUMMARY_MARK) =>
            {
                Some(text.as_str())
            }
            _ => None,
        }),
        Message::Assistant { .. } => None,
    });
    let reply = messages.iter().rev().find_map(|m| match m {
        Message::Assistant { .. } => Some(m.text()).filter(|t| !t.trim().is_empty()),
        Message::User { .. } => None,
    });
    let mut out = Vec::new();
    if let Some(p) = prompt {
        out.push(format!("› {}", clip(p.trim(), 100)));
    }
    if let Some(r) = reply {
        let lines: Vec<&str> = r
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .collect();
        let start = lines.len().saturating_sub(3);
        if start > 0 {
            out.push("  …".into());
        }
        out.extend(lines[start..].iter().map(|l| format!("  {}", clip(l, 100))));
    }
    out
}

fn term_width() -> usize {
    static WIDTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *WIDTH.get_or_init(term::width)
}

pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// A path relative to the working directory when it is inside it.
pub fn short_path(p: &str, cwd: &Path) -> String {
    let path = Path::new(p);
    match path.strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => match std::env::var("HOME") {
            Ok(home) if p.starts_with(&home) => format!("~{}", &p[home.len()..]),
            _ => p.to_string(),
        },
    }
}

/// Drops a leading `cd <working directory> &&`, which says nothing new.
fn without_cd<'a>(command: &'a str, cwd: &Path) -> &'a str {
    let dir = cwd.display().to_string();
    let Some(rest) = command.trim_start().strip_prefix("cd ") else {
        return command;
    };
    let rest = rest.trim_start();
    let rest = [format!("'{dir}'"), format!("\"{dir}\""), dir]
        .iter()
        .find_map(|d| rest.strip_prefix(d.as_str()))
        .map(str::trim_start);
    match rest.and_then(|r| r.strip_prefix("&&").or_else(|| r.strip_prefix(';'))) {
        Some(r) => r.trim_start(),
        None => command,
    }
}

fn clip(s: &str, n: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > n || s.lines().count() > 1 {
        format!("{}…", first.chars().take(n).collect::<String>())
    } else {
        first.to_string()
    }
}

/// The name and detail shown when a tool starts.
pub fn tool_title(name: &str, input: &Value, cwd: &Path) -> (String, String) {
    let s = |k: &str| input[k].as_str().unwrap_or("");
    let detail = match name {
        "Bash" => clip(without_cd(s("command"), cwd), 90),
        "Read" | "Write" | "Edit" => short_path(s("file_path"), cwd),
        "Grep" => format!(
            "{}{}",
            s("pattern"),
            match s("path") {
                "" => String::new(),
                p => format!(" in {}", short_path(p, cwd)),
            }
        ),
        "Glob" => s("pattern").to_string(),
        "WebFetch" => s("url").to_string(),
        "TaskStop" => s("task_id").to_string(),
        "TodoWrite" => String::new(),
        _ => String::new(),
    };
    let title = match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
        Some((server, tool)) => format!("{server} · {tool}"),
        None if name == "TodoWrite" => "Plan".into(),
        None => name.to_string(),
    };
    (title, detail)
}

/// Result lines under a tool header, already coloured when `color` is set.
/// Whole changed lines with their numbers and one line of context, found
/// by locating `new_string` in the edited file.
fn edit_diff(input: &Value, after: &str, color: bool) -> Option<Vec<String>> {
    let paint = |code: &str, s: &str| {
        if color {
            format!("{code}{s}{RESET}")
        } else {
            s.to_string()
        }
    };
    let (old, new) = (input["old_string"].as_str()?, input["new_string"].as_str()?);
    if new.is_empty() {
        return None;
    }
    let start = after.find(new)?;
    let end = start + new.len();
    let line_start = after[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = after[end..].find('\n').map_or(after.len(), |i| end + i);
    let (prefix, suffix) = (&after[line_start..start], &after[end..line_end]);
    let first = after[..line_start].matches('\n').count() + 1;
    let old_full = format!("{prefix}{old}{suffix}");
    let new_full = &after[line_start..line_end];
    let old_lines: Vec<&str> = old_full.lines().collect();
    let new_lines: Vec<&str> = new_full.lines().collect();
    let all: Vec<&str> = after.lines().collect();
    // (sign, line number, text): the line before, the diff, the line after.
    let mut rows: Vec<(char, usize, &str)> = Vec::new();
    if first > 1 {
        rows.push((' ', first - 1, all[first - 2]));
    }
    for op in line_diff(&old_lines, &new_lines) {
        rows.push(match op {
            Op::Same(n) => (' ', first + n, new_lines[n]),
            Op::Del(o) => ('-', first + o, old_lines[o]),
            Op::Add(n) => ('+', first + n, new_lines[n]),
        });
    }
    let next = first + new_lines.len();
    if let Some(l) = all.get(next - 1) {
        rows.push((' ', next, l));
    }
    let changed = |k: usize| rows.get(k).is_some_and(|r| r.0 != ' ');
    let width = rows
        .iter()
        .map(|r| r.1)
        .max()
        .unwrap_or(1)
        .to_string()
        .len();
    let (mut out, mut shown, mut gap) = (Vec::new(), 0, false);
    let total = rows.iter().filter(|r| r.0 != ' ').count();
    for (k, (sign, n, text)) in rows.iter().enumerate() {
        let near = changed(k) || (k > 0 && changed(k - 1)) || changed(k + 1);
        if !near {
            gap = true;
            continue;
        }
        if shown >= MAX_DIFF_LINES {
            out.push(paint(
                DIM,
                &format!("{:>width$}   … {} more changed lines", "", total - shown),
            ));
            break;
        }
        if std::mem::take(&mut gap) {
            out.push(paint(DIM, &format!("{:>width$}   ⋮", "")));
        }
        let code = match sign {
            '-' => RED,
            '+' => GREEN,
            _ => DIM,
        };
        out.push(paint(code, &format!("{n:>width$} {sign} {text}")));
        shown += usize::from(*sign != ' ');
    }
    Some(out)
}

const MAX_DIFF_LINES: usize = 16;

#[derive(Debug, PartialEq)]
enum Op {
    Same(usize),
    Del(usize),
    Add(usize),
}

/// A line diff: the longest common subsequence for blocks of a few hundred
/// lines, else common ends and a plain replacement in between.
fn line_diff(a: &[&str], b: &[&str]) -> Vec<Op> {
    let head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut ops: Vec<Op> = (0..head).map(Op::Same).collect();
    if am.len() * bm.len() <= 250_000 {
        // lcs[i][j]: common subsequence length of am[i..] and bm[j..].
        let mut lcs = vec![vec![0u32; bm.len() + 1]; am.len() + 1];
        for i in (0..am.len()).rev() {
            for j in (0..bm.len()).rev() {
                lcs[i][j] = if am[i] == bm[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < am.len() || j < bm.len() {
            if i < am.len() && j < bm.len() && am[i] == bm[j] {
                ops.push(Op::Same(head + j));
                (i, j) = (i + 1, j + 1);
            } else if j == bm.len() || (i < am.len() && lcs[i + 1][j] >= lcs[i][j + 1]) {
                ops.push(Op::Del(head + i));
                i += 1;
            } else {
                ops.push(Op::Add(head + j));
                j += 1;
            }
        }
    } else {
        ops.extend((0..am.len()).map(|i| Op::Del(head + i)));
        ops.extend((0..bm.len()).map(|j| Op::Add(head + j)));
    }
    ops.extend((b.len() - tail..b.len()).map(Op::Same));
    ops
}

pub fn tool_lines(
    name: &str,
    input: &Value,
    result: &Result<String, String>,
    // For Edit: the file after the change, to number the lines.
    after: Option<&str>,
    color: bool,
) -> Vec<String> {
    let paint = |code: &str, s: &str| {
        if color {
            format!("{code}{s}{RESET}")
        } else {
            s.to_string()
        }
    };
    let text = match result {
        // A command's output is still output; only its status is the failure.
        Err(e) if name == "Bash" => {
            let lines: Vec<&str> = e.lines().filter(|l| !l.trim().is_empty()).collect();
            let status = lines
                .last()
                .filter(|l| l.starts_with('[') && l.ends_with(']'))
                .map(|l| l.trim_matches(['[', ']']));
            let output = &lines[..lines.len() - usize::from(status.is_some())];
            let start = output.len().saturating_sub(4);
            let mut out = Vec::new();
            if start > 0 {
                out.push(paint(DIM, &format!("… {start} lines above")));
            }
            out.extend(output[start..].iter().map(|l| paint(DIM, &clip(l, 160))));
            out.push(paint(RED, &format!("✗ {}", status.unwrap_or("failed"))));
            return out;
        }
        Err(e) => {
            let lines: Vec<&str> = e.lines().filter(|l| !l.trim().is_empty()).collect();
            let shown = lines.len().saturating_sub(4);
            return lines[shown..]
                .iter()
                .map(|l| paint(RED, &format!("✗ {l}")))
                .collect();
        }
        Ok(t) => t,
    };
    let count = |s: &str| s.lines().count();
    let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    match name {
        "Read" => vec![paint(DIM, &format!("read {}", plural(count(text), "line")))],
        "Write" => vec![paint(
            DIM,
            &format!(
                "wrote {}",
                plural(count(input["content"].as_str().unwrap_or("")), "line")
            ),
        )],
        "Edit" => {
            let numbered = after
                .filter(|_| input["replace_all"] != true)
                .and_then(|a| edit_diff(input, a, color));
            if let Some(lines) = numbered {
                return lines;
            }
            let mut out = Vec::new();
            for (sign, code, key) in [("-", RED, "old_string"), ("+", GREEN, "new_string")] {
                let lines: Vec<&str> = input[key].as_str().unwrap_or("").lines().collect();
                for l in lines.iter().take(6) {
                    out.push(paint(code, &format!("{sign} {l}")));
                }
                if lines.len() > 6 {
                    out.push(paint(DIM, &format!("  … {} more", lines.len() - 6)));
                }
            }
            if input["replace_all"] == true {
                out.push(paint(DIM, text));
            }
            out
        }
        "TodoWrite" => text
            .lines()
            .map(|l| match l.get(..3) {
                Some("[x]") => paint(GREEN, &format!("✓ {}", &l[4..])),
                Some("[>]") => paint(&format!("{BOLD}{YELLOW}"), &format!("▸ {}", &l[4..])),
                Some("[ ]") => paint(DIM, &format!("○ {}", &l[4..])),
                _ => paint(DIM, l),
            })
            .collect(),
        "Grep" | "Glob" => {
            let n = count(text);
            let mut out: Vec<String> = text.lines().take(3).map(|l| paint(DIM, l)).collect();
            if n > 3 {
                out.push(paint(DIM, &format!("… {} in all", plural(n, "result"))));
            }
            out
        }
        _ => {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            if lines.is_empty() {
                return vec![paint(DIM, "(no output)")];
            }
            let start = lines.len().saturating_sub(4);
            let mut out = Vec::new();
            if start > 0 {
                out.push(paint(DIM, &format!("… {} above", plural(start, "line"))));
            }
            out.extend(lines[start..].iter().map(|l| paint(DIM, &clip(l, 160))));
            out
        }
    }
}

impl Ui for Term {
    fn waiting(&mut self, label: &str) {
        self.spin(label);
    }

    fn delta(&mut self, d: Delta) {
        match d {
            Delta::Text(t) => {
                self.unspin();
                let out = self.md.push(t);
                if !out.trim().is_empty() || self.last == Shown::Text {
                    self.shown(Shown::Text);
                    print!("{out}");
                    let _ = io::stdout().flush();
                }
            }
            Delta::Thinking => {
                self.spin("thinking");
                self.thought.clear();
            }
            Delta::ThinkingText(t) => {
                self.thought.push_str(t);
                self.tick(false);
            }
            Delta::ToolCall(name) => {
                self.settle();
                let w = Writing::new(name);
                self.spin(&w.label(&self.cwd));
                self.writing = Some(w);
            }
            Delta::ToolInput(part) => {
                if let Some(w) = &mut self.writing {
                    w.push(part);
                    let label = w.label(&self.cwd);
                    self.relabel(label);
                }
            }
        }
    }

    fn tool_start(&mut self, name: &str, input: &Value) {
        self.writing = None;
        self.settle();
        self.shown(Shown::Tool);
        let (title, detail) = tool_title(name, input, &self.cwd);
        eprintln!(
            "{} {}{}",
            self.paint(CYAN, "●"),
            self.paint(BOLD, &title),
            if detail.is_empty() {
                String::new()
            } else {
                format!(" {}", self.dim(&detail))
            }
        );
        self.tool = Some((name.to_string(), input.clone()));
        let label = if detail.is_empty() {
            title
        } else {
            format!("{title} {detail}")
        };
        self.spin(&label);
    }

    fn tool_end(&mut self, result: &Result<String, String>) {
        self.unspin();
        let (name, input) = self.tool.take().unwrap_or_default();
        let after = (name == "Edit" && result.is_ok())
            .then(|| {
                input["file_path"]
                    .as_str()
                    .and_then(|p| std::fs::read_to_string(p).ok())
            })
            .flatten();
        for line in tool_lines(&name, &input, result, after.as_deref(), self.color) {
            eprintln!("  {line}");
        }
    }

    fn stopped(&mut self, stop: &Stop) {
        self.settle();
        let note = match stop {
            Stop::EndTurn | Stop::ToolUse => return,
            Stop::MaxTokens => "stopped: the reply hit the output limit".to_string(),
            Stop::Refusal => "stopped: the model declined".to_string(),
            Stop::Other(r) => format!("stopped: {r}"),
        };
        eprintln!("{}", self.paint(YELLOW, &note));
    }

    fn usage(&mut self, u: Usage) {
        self.turn.output += u.output;
        self.total_out += u.output;
        self.turn.input = u.input;
        self.turn.cache_read = u.cache_read;
        self.turn.cache_write = u.cache_write;
    }

    fn interrupted(&mut self) -> bool {
        if self.tool.is_some()
            && let Some(line) = crate::tools::latest_output_line()
        {
            self.thought = line;
        }
        self.tick(false);
        self.keys.as_mut().is_some_and(|k| k.stop_requested())
    }

    fn ask(&mut self, question: &str) -> bool {
        self.settle();
        let mark = self.paint(YELLOW, "?");
        let Some(keys) = &mut self.keys else {
            return false;
        };
        eprint!("{mark} {question} [y/N] ");
        let _ = io::stderr().flush();
        let yes = matches!(keys.next(), Some(term::Key::Char('y' | 'Y')));
        eprintln!("{}", if yes { "yes" } else { "no" });
        yes
    }

    fn notice(&mut self, text: &str) {
        self.settle();
        eprintln!("{}", self.dim(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn titles_are_short_and_relative() {
        let cwd = Path::new("/work/proj");
        assert_eq!(
            tool_title("Read", &json!({"file_path": "/work/proj/src/lib.rs"}), cwd),
            ("Read".into(), "src/lib.rs".into())
        );
        assert_eq!(
            tool_title("Bash", &json!({"command": "cargo test\n--more"}), cwd),
            ("Bash".into(), "cargo test…".into())
        );
        assert_eq!(
            tool_title("mcp__github__create_issue", &json!({}), cwd).0,
            "github · create_issue"
        );
        assert_eq!(tool_title("TodoWrite", &json!({}), cwd).0, "Plan");
        let bash = |c: &str| tool_title("Bash", &json!({"command": c}), cwd).1;
        assert_eq!(bash("cd /work/proj && ls -la"), "ls -la");
        assert_eq!(bash("cd '/work/proj'; make"), "make");
        assert_eq!(
            bash("cd /work/proj/sub && make"),
            "cd /work/proj/sub && make"
        );
        assert_eq!(bash("cd /work/proj"), "cd /work/proj");
    }

    #[test]
    fn streaming_tool_input_shows_progress() {
        let cwd = Path::new("/w");
        let mut w = Writing::new("Write");
        assert_eq!(w.label(cwd), "writing Write");
        w.push(r#"{"file_path": "/w/a.s", "content": "x\\nliteral\n"#);
        w.push(r#"y\"#);
        w.push(r#"nz"#);
        assert_eq!(w.label(cwd), "writing Write a.s · 3 lines");
        assert_eq!(string_field(r#"{"command": "ls \"a"#, "command"), None);
        assert_eq!(
            string_field(r#"{"command": "ls \"a\""}"#, "command").as_deref(),
            Some(r#"ls "a""#)
        );
    }

    #[test]
    fn recap_shows_the_last_prompt_and_reply_end() {
        use crate::message::{Block, Message};
        let messages = vec![
            Message::user_text("first ask"),
            Message::Assistant {
                content: vec![Block::Text {
                    text: "one\ntwo\n\nthree\nfour".into(),
                }],
            },
            Message::user_text("fix the tests\nplease"),
            Message::Assistant { content: vec![] },
            Message::User {
                content: vec![Block::ToolResult {
                    id: "1".into(),
                    output: "x".into(),
                    is_error: false,
                }],
            },
            Message::user_text(crate::compact::INTERRUPTED),
        ];
        assert_eq!(
            recap(&messages),
            ["› fix the tests…", "  …", "  two", "  three", "  four"]
        );
        assert!(recap(&[]).is_empty());
    }

    #[test]
    fn thinking_note_keeps_the_newest_words() {
        assert_eq!(
            latest_line("**Planning**\n\nChecking the compiler", 40).as_deref(),
            Some("Checking the compiler")
        );
        assert_eq!(
            latest_line("abcdefghijklmnopqrstuvwxyz", 12).as_deref(),
            Some("…pqrstuvwxyz")
        );
        assert_eq!(latest_line("anything", 5), None);
        assert_eq!(latest_line("\n  \n", 40), None);
    }

    #[test]
    fn line_diff_keeps_unchanged_lines_out_of_the_change() {
        use Op::*;
        assert_eq!(
            line_diff(&["a", "b", "c"], &["a", "B", "c", "d"]),
            [Same(0), Del(1), Add(1), Same(2), Add(3)]
        );
        let after = "x\ndef f(n=1):\n    \"\"\"Doc.\n\n    More.\n    \"\"\"\n    return n\n";
        let input = json!({
            "old_string": "def f():\n    \"\"\"Doc.\n\n    More.\n    \"\"\"\n    return 1",
            "new_string": "def f(n=1):\n    \"\"\"Doc.\n\n    More.\n    \"\"\"\n    return n"
        });
        assert_eq!(
            tool_lines("Edit", &input, &Ok("ok".into()), Some(after), false),
            [
                "1   x",
                "2 - def f():",
                "2 + def f(n=1):",
                "3       \"\"\"Doc.",
                "    ⋮",
                "6       \"\"\"",
                "7 -     return 1",
                "7 +     return n",
            ]
        );
    }

    #[test]
    fn edits_show_numbered_lines_with_context() {
        let after = "fn main() {\n    let x = 2;\n    println!(\"{x}\");\n}\n";
        let input = json!({"old_string": "1", "new_string": "2"});
        assert_eq!(
            tool_lines("Edit", &input, &Ok("edited".into()), Some(after), false),
            [
                "1   fn main() {",
                "2 -     let x = 1;",
                "2 +     let x = 2;",
                "3       println!(\"{x}\");"
            ]
        );
        let added = json!({"old_string": "}", "new_string": "    done();\n}"});
        let after = "fn f() {\n    done();\n}";
        assert_eq!(
            tool_lines("Edit", &added, &Ok("edited".into()), Some(after), false),
            ["1   fn f() {", "2 +     done();", "3   }"]
        );
    }

    #[test]
    fn results_are_summarized() {
        let edit = json!({"old_string": "a\nb", "new_string": "c"});
        assert_eq!(
            tool_lines("Edit", &edit, &Ok("edited x".into()), None, false),
            ["- a", "- b", "+ c"]
        );
        assert_eq!(
            tool_lines("Read", &json!({}), &Ok("1\n2\n3".into()), None, false),
            ["read 3 lines"]
        );
        assert_eq!(
            tool_lines(
                "TodoWrite",
                &json!({}),
                &Ok("[x] one\n[>] two\n[ ] three".into()),
                None,
                false
            ),
            ["✓ one", "▸ two", "○ three"]
        );
        let long = (1..=10)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            tool_lines("Bash", &json!({}), &Ok(long), None, false),
            ["… 6 lines above", "7", "8", "9", "10"]
        );
        assert_eq!(
            tool_lines("Bash", &json!({}), &Ok(String::new()), None, false),
            ["(no output)"]
        );
        assert_eq!(
            tool_lines(
                "Bash",
                &json!({}),
                &Err("boom\n[exit code 1]".into()),
                None,
                false
            ),
            ["boom", "✗ exit code 1"]
        );
    }
}
