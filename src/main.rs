mod agent;
mod context;
mod message;
mod provider;
mod session;
mod sse;
mod tools;

use agent::{Agent, Ui};
use provider::{Delta, Model, Stop, Usage};
use serde_json::Value;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

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

Interactive commands: /new, /model <model>, /session, /quit

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

    fn usage(&mut self, _: Usage) {}
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

    eprintln!(
        "{}",
        term.dim(&format!(
            "axio {} · {} · /quit to exit",
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
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        if prompt.is_empty() {
            eprint!("\n› ");
            let _ = io::stderr().flush();
            match lines.next() {
                Some(Ok(line)) => prompt = line,
                _ => break,
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
                if agent.session.is_some() {
                    agent.session = Some(session::Session::create(&sessions, &cwd)?);
                }
                eprintln!("{}", term.dim("new session"));
            }
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
                if let Err(e) = agent.prompt(input, &mut term) {
                    eprintln!("\naxio: {e}");
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
