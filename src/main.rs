mod agent;
mod compact;
mod context;
mod editor;
mod message;
mod provider;
mod session;
mod sse;
mod term;
mod tools;

use agent::{Agent, Ui};
use provider::{Delta, Model, Stop, Usage};
use serde_json::Value;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "axio: a minimal coding agent

Usage: axio [options] [prompt...]

Options:
  -p, --print           Answer one prompt (argument or stdin) and exit
  -m, --model <model>   provider/model, e.g. anthropic/claude-opus-5-5,
                        openai/<id>, openrouter/<vendor>/<id> (env AXIO_MODEL)
  -e, --effort <level>  Reasoning effort sent to the provider (env AXIO_EFFORT)
  -c, --continue        Continue the most recent session in this directory
  -s, --session <file>  Continue a session file
      --no-session      Do not save this session
  -h, --help            Show this help
  -V, --version         Show the version

Interactive: Enter sends, Alt-Enter or a trailing \\ adds a line, Up/Down
walk history, Esc or Ctrl-C stops a running turn, Ctrl-D exits.
Commands: /new, /compact, /model <model>, /session, /quit

Credentials come from the environment: ANTHROPIC_API_KEY or
ANTHROPIC_AUTH_TOKEN, OPENAI_API_KEY, OPENROUTER_API_KEY. Any other provider
name reads <NAME>_BASE_URL and <NAME>_API_KEY.";

#[derive(Default)]
struct Args {
    print: bool,
    model: Option<String>,
    effort: Option<String>,
    resume_latest: bool,
    session: Option<PathBuf>,
    no_session: bool,
    prompt: Vec<String>,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("axio {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-p" | "--print" => a.print = true,
            "-m" | "--model" => a.model = Some(value(&arg)?),
            "-e" | "--effort" => a.effort = Some(value(&arg)?),
            "-c" | "--continue" => a.resume_latest = true,
            "-s" | "--session" => a.session = Some(value(&arg)?.into()),
            "--no-session" => a.no_session = true,
            "--" => a.prompt.extend(it.by_ref()),
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {s}")),
            _ => a.prompt.push(arg),
        }
    }
    Ok(Some(a))
}

struct Term {
    color: bool,
    thinking: bool,
    keys: Option<term::Keys>,
    turn: Usage,
}

impl Term {
    fn dim(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[2m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

impl Ui for Term {
    fn delta(&mut self, d: Delta) {
        match d {
            Delta::Text(t) => {
                self.thinking = false;
                print!("{t}");
                let _ = io::stdout().flush();
            }
            Delta::Thinking if !self.thinking => {
                self.thinking = true;
                eprintln!("{}", self.dim("thinking…"));
            }
            Delta::Thinking | Delta::ToolCall => {}
        }
    }

    fn tool_start(&mut self, name: &str, input: &Value) {
        let detail = match name {
            "bash" => input["command"].as_str().unwrap_or("").to_string(),
            _ => input["path"].as_str().unwrap_or("").to_string(),
        };
        let first = detail.lines().next().unwrap_or("");
        eprintln!("\n{}", self.dim(&format!("→ {name} {first}")));
    }

    fn tool_end(&mut self, result: &Result<String, String>) {
        let (mark, text) = match result {
            Ok(t) => ("✓", t),
            Err(e) => ("✗", e),
        };
        let lines: Vec<&str> = text.lines().collect();
        let shown = lines
            .iter()
            .rev()
            .take(3)
            .rev()
            .copied()
            .collect::<Vec<_>>();
        let more = if lines.len() > 3 {
            format!(" (+{} lines)", lines.len() - 3)
        } else {
            String::new()
        };
        eprintln!(
            "{}",
            self.dim(&format!("{mark}{more} {}", shown.join("\n  ")))
        );
    }

    fn stopped(&mut self, stop: &Stop) {
        println!();
        match stop {
            Stop::EndTurn | Stop::ToolUse => {}
            Stop::MaxTokens => eprintln!("{}", self.dim("[stopped: output limit]")),
            Stop::Refusal => eprintln!("{}", self.dim("[stopped: the model declined]")),
            Stop::Other(r) => eprintln!("{}", self.dim(&format!("[stopped: {r}]"))),
        }
    }

    fn usage(&mut self, u: Usage) {
        self.turn.output += u.output;
        self.turn.input = u.input;
        self.turn.cache_read = u.cache_read;
        self.turn.cache_write = u.cache_write;
    }

    fn interrupted(&mut self) -> bool {
        self.keys.as_mut().is_some_and(|k| k.stop_requested())
    }

    fn notice(&mut self, text: &str) {
        eprintln!("{}", self.dim(text));
    }
}

fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

fn status(agent: &Agent, turn: &Usage) -> String {
    let sent = turn.input + turn.cache_read + turn.cache_write;
    let cached = (turn.cache_read * 100)
        .checked_div(sent)
        .map(|p| format!(" · {p}% cached"))
        .unwrap_or_default();
    format!(
        "context {}/{} · {} out{cached}",
        tokens(agent.context),
        tokens(agent.model.context_window()),
        tokens(turn.output)
    )
}

fn history_path() -> std::path::PathBuf {
    context::home().join("history.jsonl")
}

fn load_history() -> Vec<String> {
    let text = std::fs::read_to_string(history_path()).unwrap_or_default();
    let mut all: Vec<String> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let skip = all.len().saturating_sub(500);
    all.drain(..skip);
    all
}

fn save_history(entry: &str) {
    let path = history_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{}", serde_json::Value::String(entry.into()));
    }
}

enum Input {
    Line(String),
    Exit,
}

/// Edits one prompt until Enter. Ctrl-C twice on an empty line, or Ctrl-D,
/// exits.
fn read_prompt(editor: &mut editor::Editor, keys: &mut term::Keys) -> Input {
    let mut width = term::width();
    let mut drawn = editor.layout(width);
    print!("{}", drawn.redraw(0));
    let _ = io::stdout().flush();
    let mut last_interrupt: Option<Instant> = None;
    loop {
        let Some(key) = keys.next() else {
            return Input::Exit;
        };
        let outcome = editor.handle(key);
        let previous = drawn.cursor_row;
        match outcome {
            editor::Outcome::Submit(text) => {
                // Leave the submitted text on screen, then start below it.
                let mut done = editor::Editor::default();
                done.handle(term::Key::Paste(text.clone()));
                println!("{}", done.layout(width).redraw(previous));
                let _ = io::stdout().flush();
                return Input::Line(text);
            }
            editor::Outcome::Exit => {
                println!();
                return Input::Exit;
            }
            editor::Outcome::Interrupt => {
                if last_interrupt.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
                    println!();
                    return Input::Exit;
                }
                last_interrupt = Some(Instant::now());
                print!("\r\x1b[J\x1b[2m(Ctrl-C again or Ctrl-D to exit)\x1b[0m\n");
                drawn = editor.layout(width);
                print!("{}", drawn.redraw(0));
                let _ = io::stdout().flush();
                continue;
            }
            editor::Outcome::Redraw => {
                width = term::width();
                print!("\x1b[2J\x1b[H");
                drawn = editor.layout(width);
                print!("{}", drawn.redraw(0));
                let _ = io::stdout().flush();
                continue;
            }
            editor::Outcome::Editing => {}
        }
        drawn = editor.layout(width);
        print!("{}", drawn.redraw(previous));
        let _ = io::stdout().flush();
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("axio: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let Some(args) = parse_args()? else {
        return Ok(ExitCode::SUCCESS);
    };
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let spec = args
        .model
        .or_else(|| std::env::var("AXIO_MODEL").ok())
        .unwrap_or_else(|| provider::DEFAULT_MODEL.into());
    let mut agent = Agent::new(
        Model::parse(&spec),
        cwd.clone(),
        context::system_prompt(&cwd),
    );
    agent.effort = args.effort.or_else(|| std::env::var("AXIO_EFFORT").ok());

    let sessions = session::dir_for(&context::home(), &cwd);
    let resume = match (args.session, args.resume_latest) {
        (Some(p), _) => Some(p),
        (None, true) => Some(session::latest(&sessions).ok_or("no session to continue here")?),
        _ => None,
    };
    if let Some(path) = resume {
        agent.messages = session::load(&path)?;
        agent.session = Some(session::Session::open(path)?);
    } else if !args.no_session {
        agent.session = Some(session::Session::create(&sessions, &cwd)?);
    }

    let mut term = Term {
        color: io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        thinking: false,
        keys: None,
        turn: Usage::default(),
    };

    let mut prompt = args.prompt.join(" ");
    if args.print || !io::stdin().is_terminal() {
        if prompt.is_empty() || !io::stdin().is_terminal() {
            let mut piped = String::new();
            io::stdin()
                .read_to_string(&mut piped)
                .map_err(|e| e.to_string())?;
            prompt = [prompt, piped].join("\n").trim().to_string();
        }
        if prompt.is_empty() {
            return Err("no prompt".into());
        }
        agent.prompt(&prompt, &mut term)?;
        return Ok(ExitCode::SUCCESS);
    }

    let _raw = term::RawMode::enter().ok_or("could not configure the terminal (stty failed)")?;
    term.keys = Some(term::Keys::spawn());
    eprintln!(
        "{}",
        term.dim(&format!(
            "axio {} · {} · Esc stops a turn · /quit to exit",
            env!("CARGO_PKG_VERSION"),
            agent.model
        ))
    );
    if !agent.messages.is_empty() {
        eprintln!(
            "{}",
            term.dim(&format!("resumed {} messages", agent.messages.len()))
        );
    }
    let mut editor = editor::Editor::default();
    editor.history = load_history();
    loop {
        if prompt.is_empty() {
            println!();
            let keys = term.keys.as_mut().expect("keys");
            match read_prompt(&mut editor, keys) {
                Input::Line(line) => {
                    save_history(&line);
                    prompt = line;
                }
                Input::Exit => break,
            }
        }
        let input = std::mem::take(&mut prompt);
        let input = input.trim();
        match input
            .split_once(' ')
            .map_or((input, ""), |(c, r)| (c, r.trim()))
        {
            ("", _) => continue,
            ("/quit" | "/exit", _) => break,
            ("/new", _) => {
                agent.messages.clear();
                agent.context = 0;
                if agent.session.is_some() {
                    agent.session = Some(session::Session::create(&sessions, &cwd)?);
                }
                eprintln!("{}", term.dim("new session"));
            }
            ("/compact", _) => match agent.compact(&mut term) {
                Ok(true) => eprintln!("{}", term.dim("compacted")),
                Ok(false) => eprintln!("{}", term.dim("nothing older to compact")),
                Err(e) => eprintln!("axio: {e}"),
            },
            ("/model", "") => eprintln!("{}", agent.model),
            ("/model", m) => {
                agent.model = Model::parse(m);
                eprintln!("{}", term.dim(&format!("model: {}", agent.model)));
            }
            ("/session", _) => match &agent.session {
                Some(s) => eprintln!("{}", s.path.display()),
                None => eprintln!("not saved"),
            },
            _ => {
                term.turn = Usage::default();
                term.thinking = false;
                match agent.prompt(input, &mut term) {
                    Ok(()) => {}
                    Err(e) if e == agent::ABORTED => eprintln!("\n{}", term.dim("[stopped]")),
                    Err(e) => eprintln!("\naxio: {e}"),
                }
                eprintln!("{}", term.dim(&status(&agent, &term.turn)));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
