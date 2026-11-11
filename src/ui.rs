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
    cwd: PathBuf,
    md: Markdown,
    spinner: Option<(String, Instant)>,
    drawn: bool,
    last_frame: Instant,
    frame: usize,
    tool: Option<(String, Value)>,
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
            cwd: cwd.to_path_buf(),
            md: Markdown::new(color_out),
            spinner: None,
            drawn: false,
            last_frame: Instant::now(),
            frame: 0,
            tool: None,
            writing: None,
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
        let text: String = format!("{label}{time}").chars().take(width).collect();
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
        "Bash" => clip(s("command"), 90),
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
pub fn tool_lines(
    name: &str,
    input: &Value,
    result: &Result<String, String>,
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
            Delta::Thinking => self.spin("thinking"),
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
        for line in tool_lines(&name, &input, result, self.color) {
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
        self.turn.input = u.input;
        self.turn.cache_read = u.cache_read;
        self.turn.cache_write = u.cache_write;
    }

    fn interrupted(&mut self) -> bool {
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
    fn results_are_summarized() {
        let edit = json!({"old_string": "a\nb", "new_string": "c"});
        assert_eq!(
            tool_lines("Edit", &edit, &Ok("edited x".into()), false),
            ["- a", "- b", "+ c"]
        );
        assert_eq!(
            tool_lines("Read", &json!({}), &Ok("1\n2\n3".into()), false),
            ["read 3 lines"]
        );
        assert_eq!(
            tool_lines(
                "TodoWrite",
                &json!({}),
                &Ok("[x] one\n[>] two\n[ ] three".into()),
                false
            ),
            ["✓ one", "▸ two", "○ three"]
        );
        let long = (1..=10)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            tool_lines("Bash", &json!({}), &Ok(long), false),
            ["… 6 lines above", "7", "8", "9", "10"]
        );
        assert_eq!(
            tool_lines("Bash", &json!({}), &Ok(String::new()), false),
            ["(no output)"]
        );
        assert_eq!(
            tool_lines(
                "Bash",
                &json!({}),
                &Err("boom\n[exit code 1]".into()),
                false
            ),
            ["✗ boom", "✗ [exit code 1]"]
        );
    }
}
