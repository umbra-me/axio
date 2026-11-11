mod agent;
mod auth;
mod background;
mod compact;
mod context;
mod crypto;
mod editor;
mod hooks;
mod import;
mod mcp;
mod message;
mod oauth;
mod plan;
mod plugin_cli;
mod plugins;
mod profile;
mod provider;
mod sandbox;
mod search;
mod session;
mod settings;
mod skills;
mod sse;
mod term;
mod toml_lite;
mod tools;
mod web;

use agent::{Agent, Ui};
use provider::{Delta, Model, Stop, Usage};
use serde_json::Value;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "axio: a minimal coding agent

Usage: axio [options] [prompt...]
       axio plugin install|list|update|remove ...
       axio import   (review what other harnesses offer again)
       axio mcp list | login <name> | logout <name>
       axio auth list | set <provider> [field] | remove <provider>
       axio profile [name]   (show or set the default profile)

ax is the same as axio; axio-full, axio-min (or any axio-<profile>) start
that profile.

Options:
  -p, --print           Answer one prompt (argument or stdin) and exit
  -m, --model <model>   provider/model, e.g. anthropic/claude-opus-5-5,
                        openai/<id>, openrouter/<vendor>/<id> (env AXIO_MODEL)
  -e, --effort <level>  Reasoning effort sent to the provider (env AXIO_EFFORT)
  -c, --continue        Continue the most recent session in this directory
  -s, --session <file>  Continue a session file
      --no-session      Do not save this session
      --profile <name>  minimal, standard (default), full or one in
                        ~/.axio/agent/profiles/ (env AXIO_PROFILE)
      --trust           Trust this project's configuration without asking
      --no-project-config  Ignore this project's .axio/ and .mcp.json
      --no-import       Skip hooks, plugins and servers from other harnesses
  -h, --help            Show this help
  -V, --version         Show the version

Interactive: Enter sends, Alt-Enter or a trailing \\ adds a line, Up/Down
walk history, Esc or Ctrl-C stops a running turn, Ctrl-D exits.
Commands: /new, /compact, /model <model>, /session, /help, /quit, plus your own

Credentials come from the environment (ANTHROPIC_API_KEY or
ANTHROPIC_AUTH_TOKEN, OPENAI_API_KEY, OPENROUTER_API_KEY, <NAME>_BASE_URL and
<NAME>_API_KEY for other providers) or from ~/.axio/agent/auth.json, which
`axio auth set <provider>` writes.";

#[derive(Default)]
struct Args {
    print: bool,
    model: Option<String>,
    effort: Option<String>,
    resume_latest: bool,
    session: Option<PathBuf>,
    no_session: bool,
    trust: bool,
    profile: Option<String>,
    no_project_config: bool,
    no_import: bool,
    /// `axio mcp …`, run after configuration loads.
    mcp_command: Option<Vec<String>>,
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
            "--trust" => a.trust = true,
            "--profile" => a.profile = Some(value(&arg)?),
            "--no-project-config" => a.no_project_config = true,
            "--no-import" => a.no_import = true,
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
            "Bash" => input["command"].as_str().unwrap_or("").to_string(),
            _ => input["file_path"].as_str().unwrap_or("").to_string(),
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

    fn ask(&mut self, question: &str) -> bool {
        let Some(keys) = &mut self.keys else {
            return false;
        };
        eprint!("\n{question} [y/N] ");
        let _ = io::stderr().flush();
        let yes = matches!(keys.next(), Some(term::Key::Char('y' | 'Y')));
        eprintln!("{}", if yes { "yes" } else { "no" });
        yes
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

/// The project root when its configuration may load. Asks once per project in
/// interactive mode; `-p` runs only use projects trusted earlier or `--trust`.
fn trusted_project(
    home: &std::path::Path,
    cwd: &std::path::Path,
    args: &Args,
    interactive: bool,
) -> Result<Option<std::path::PathBuf>, String> {
    let root = settings::project_root(cwd);
    let files = settings::project_config(&root);
    if args.no_project_config || files.is_empty() {
        return Ok(None);
    }
    let mut trust = settings::Trust::load(home);
    let trusted = match trust.decision(&root) {
        settings::Decision::Trusted => true,
        _ if args.trust => {
            trust.record(home, &root, true)?;
            true
        }
        settings::Decision::Declined => false,
        settings::Decision::Unknown if interactive => {
            eprintln!("This project has configuration that can make axio run programs:");
            for f in &files {
                eprintln!("  {}", f.display());
            }
            eprint!("Trust {}? [y/N] ", root.display());
            let _ = io::stderr().flush();
            let mut answer = String::new();
            io::stdin()
                .read_line(&mut answer)
                .map_err(|e| e.to_string())?;
            let yes = matches!(answer.trim(), "y" | "Y" | "yes");
            trust.record(home, &root, yes)?;
            yes
        }
        settings::Decision::Unknown => false,
    };
    if !trusted {
        eprintln!(
            "axio: ignoring this project's configuration (not trusted; run with --trust to allow it)"
        );
    }
    Ok(trusted.then_some(root))
}

/// Where configuration comes from: `~/.axio/agent`, then a trusted project,
/// with plugins limited to those the profile allows.
struct Sources<'a> {
    home: &'a std::path::Path,
    project: Option<&'a std::path::Path>,
    allowed: Option<&'a [String]>,
    /// Accepted items from other harnesses, used in place.
    imported: Vec<import::Item>,
}

impl Sources<'_> {
    fn imported_plugins(&self) -> impl Iterator<Item = (&str, &std::path::Path, bool)> {
        self.imported
            .iter()
            .filter_map(|i| match &i.kind {
                import::Kind::Plugin { root, codex } => {
                    Some((i.name.as_str(), root.as_path(), *codex))
                }
                _ => None,
            })
            .filter(|(name, _, _)| {
                self.allowed
                    .is_none_or(|a| a.iter().any(|n| n == "*" || n == name))
            })
    }

    fn plugins(&self, dir: std::path::PathBuf) -> Vec<plugins::Plugin> {
        plugins::list(&dir, self.allowed)
    }

    /// Hooks from global settings and plugins, then a trusted project's.
    fn hooks(&self, cwd: &std::path::Path) -> Result<hooks::Hooks, String> {
        let mut h = hooks::Hooks::new(self.project.unwrap_or(cwd).to_path_buf());
        let mut add = |settings: std::path::PathBuf, dir: std::path::PathBuf| {
            h.add_file(&settings, None)?;
            for p in self.plugins(dir) {
                h.add_file(&p.root.join("hooks/hooks.json"), Some(&p.root))?;
            }
            Ok::<_, String>(())
        };
        add(self.home.join("settings.json"), self.home.join("plugins"))?;
        if let Some(root) = self.project {
            add(root.join(".axio/settings.json"), root.join(".axio/plugins"))?;
        }
        for i in &self.imported {
            if let import::Kind::Hooks(file) = &i.kind {
                h.add_file(file, None)?;
            }
        }
        for (_, root, codex) in self.imported_plugins() {
            let file = if codex {
                root.join(".codex-plugin/hooks.json")
            } else {
                root.join("hooks/hooks.json")
            };
            h.add_file(&file, Some(root))?;
        }
        Ok(h)
    }

    /// MCP servers from `mcp.json` and plugins, then a trusted project's.
    fn mcp(&self) -> Result<mcp::Config, String> {
        let mut c = mcp::Config::default();
        let mut add = |file: std::path::PathBuf, dir: std::path::PathBuf| {
            c.add_file(&file, None)?;
            for p in self.plugins(dir) {
                c.add_file(&p.root.join(".mcp.json"), Some(&p.root))?;
            }
            Ok::<_, String>(())
        };
        add(self.home.join("mcp.json"), self.home.join("plugins"))?;
        if let Some(root) = self.project {
            add(root.join(".mcp.json"), root.join(".axio/plugins"))?;
        }
        for i in &self.imported {
            if let import::Kind::Mcp(entry) = &i.kind {
                let source = std::path::PathBuf::from(format!("{} (imported)", i.harness));
                c.add(
                    &serde_json::json!({"mcpServers": {i.name.clone(): entry}}),
                    None,
                    &source,
                );
            }
        }
        for (_, root, codex) in self.imported_plugins() {
            let file = if codex {
                root.join(".codex-plugin/mcp.json")
            } else {
                root.join(".mcp.json")
            };
            c.add_file(&file, Some(root))?;
        }
        Ok(c)
    }

    /// Skills and commands: a trusted project's first, so it can override
    /// global ones, then `~/.axio/agent` and its plugins.
    fn library(&self) -> skills::Library {
        let mut lib = skills::Library::default();
        if let Some(root) = self.project {
            lib.add_dir(&root.join(".axio"), None);
            for p in self.plugins(root.join(".axio/plugins")) {
                lib.add_dir(&p.root, Some(&p.name));
            }
        }
        lib.add_dir(self.home, None);
        for p in self.plugins(self.home.join("plugins")) {
            lib.add_dir(&p.root, Some(&p.name));
        }
        for i in &self.imported {
            if let import::Kind::Skills(base) = &i.kind {
                lib.add_dir(base, None);
            }
        }
        for (name, root, _) in self.imported_plugins() {
            lib.add_dir(root, Some(name));
        }
        lib
    }
}

/// `/name args` for a command or invocable skill. `None` for ordinary text,
/// including paths such as `/usr/bin/x`.
fn command(lib: &skills::Library, input: &str) -> Option<Result<String, String>> {
    let rest = input.strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(
        lib.expand(name, args.trim())
            .ok_or_else(|| format!("unknown command /{name}; /help lists them")),
    )
}

fn help(lib: &skills::Library) -> String {
    let mut s = String::from(
        "/new  start a new session\n/profile  show the profile, modules and plugins\n/compact  summarize the conversation now\n/model [provider/model]  show or switch the model\n/session  show the session file\n/help  this list\n/quit  exit",
    );
    for c in &lib.commands {
        s.push_str(&format!("\n/{}  {}", c.name, c.description));
    }
    for k in lib.skills.iter().filter(|k| k.invocable) {
        s.push_str(&format!(
            "\n/{}  {} (skill)",
            k.name,
            k.description.chars().take(80).collect::<String>()
        ));
    }
    s
}

/// Items from other harnesses to use this session. Asks about new or
/// changed ones in interactive mode; `review` asks about everything again.
fn imports(
    home: &std::path::Path,
    interactive: bool,
    review: bool,
) -> Result<Vec<import::Item>, String> {
    let Some(user_home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return Ok(Vec::new());
    };
    let items = import::scan(&user_home);
    let mut record = import::Record::load(home);
    if review {
        record.never = false;
    }
    let pending: Vec<&import::Item> = if review {
        items.iter().collect()
    } else {
        record.pending(&items)
    };
    if interactive && !record.never && !pending.is_empty() {
        let cache = std::cell::RefCell::new(std::collections::HashMap::new());
        let versions = |h: &str| {
            cache
                .borrow_mut()
                .entry(h.to_string())
                .or_insert_with(|| import::harness_version(h, &user_home))
                .clone()
        };
        eprint!("{}", import::render(&pending, &versions));
        eprintln!("\n[a] import everything that works  [c] choose  [l] later  [n] never ask");
        eprintln!(
            "Turn this off any time: \"importFromOtherHarnesses\": \"off\" in {}",
            home.join("settings.json").display()
        );
        let ask = |q: &str| -> Result<String, String> {
            eprint!("{q}");
            let _ = io::stderr().flush();
            let mut a = String::new();
            io::stdin().read_line(&mut a).map_err(|e| e.to_string())?;
            Ok(a.trim().to_lowercase())
        };
        let usable = |i: &import::Item| !matches!(i.support, import::Support::No(_));
        match ask("> ")?.as_str() {
            "a" => pending.iter().for_each(|i| record.answer(i, usable(i))),
            "c" => {
                for i in &pending {
                    let yes = usable(i)
                        && matches!(
                            ask(&format!("Use {} {} {}? [y/N] ", i.harness, i.label, i.name))?
                                .as_str(),
                            "y" | "yes"
                        );
                    record.answer(i, yes);
                }
            }
            "n" => record.never = true,
            _ => eprintln!("asking again next time; `axio import` reviews this any time"),
        }
        record.save(home)?;
    }
    Ok(record.accepted(&items).into_iter().cloned().collect())
}

/// `axio mcp list | login <name> | logout <name>`.
fn mcp_command(sub: &[String], servers: &mcp::Config) -> Result<(), String> {
    let find = |name: &str| {
        servers
            .servers
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("no MCP server named {name}; `axio mcp list` shows them"))
    };
    let remote = |name: &str| -> Result<String, String> {
        find(name)?
            .url
            .clone()
            .ok_or_else(|| format!("{name} is a local server; only HTTP servers sign in"))
    };
    match sub
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["list"] | [] => {
            for s in &servers.servers {
                let how = match &s.url {
                    Some(url) => format!(
                        "{url}{}",
                        if oauth::signed_in(url) {
                            " (signed in)"
                        } else {
                            ""
                        }
                    ),
                    None => [s.command.as_str()]
                        .into_iter()
                        .chain(s.args.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" "),
                };
                println!("{}  {how}", s.name);
            }
            for skipped in &servers.skipped {
                println!("skipped: {skipped}");
            }
            Ok(())
        }
        ["login", name] => {
            let url = remote(name)?;
            oauth::login(&url, &oauth::open_browser)?;
            println!("signed in to {name}");
            Ok(())
        }
        ["logout", name] => {
            let url = remote(name)?;
            if oauth::logout(&url)? {
                println!("signed out of {name}");
            } else {
                println!("{name} was not signed in");
            }
            Ok(())
        }
        _ => Err("usage: axio mcp list | login <name> | logout <name>".into()),
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
    let all: Vec<String> = std::env::args().skip(1).collect();
    if all.first().is_some_and(|a| a == "import") {
        let accepted = imports(&context::home(), true, true)?;
        println!("{} item(s) from other harnesses in use", accepted.len());
        return Ok(ExitCode::SUCCESS);
    }
    if all.first().is_some_and(|a| a == "profile") {
        let home = context::home();
        match all.get(1) {
            Some(name) => println!(
                "default profile is now {}",
                profile::set_default(&home, name)?
            ),
            None => {
                let current = settings::Settings::load(&home, None)?
                    .profile
                    .unwrap_or_else(|| profile::DEFAULT.into());
                println!("default profile: {current}");
                println!(
                    "built in: minimal (min), standard, full; your own: ~/.axio/agent/profiles/<name>.json"
                );
                println!("one run: axio --profile <name>, or the axio-<name> / ax-<name> commands");
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    if all.first().is_some_and(|a| a == "auth") {
        auth::main(&all[1..], &context::home())?;
        return Ok(ExitCode::SUCCESS);
    }
    if all.first().is_some_and(|a| a == "plugin") {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        plugin_cli::main(&all[1..], &context::home(), &cwd)?;
        return Ok(ExitCode::SUCCESS);
    }
    let args = if all.first().is_some_and(|a| a == "mcp") {
        Args {
            mcp_command: Some(all[1..].to_vec()),
            print: true,
            no_session: true,
            ..Args::default()
        }
    } else {
        let Some(args) = parse_args()? else {
            return Ok(ExitCode::SUCCESS);
        };
        args
    };
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let home = context::home();
    oauth::set_home(&home);
    let interactive = !args.print && io::stdin().is_terminal();
    let project = trusted_project(&home, &cwd, &args, interactive)?;
    let config = settings::Settings::load(&home, project.as_deref())?;
    let profile_name = args
        .profile
        .clone()
        .or_else(|| {
            std::env::args()
                .next()
                .and_then(|a| profile::from_program(&a))
        })
        .or_else(|| std::env::var("AXIO_PROFILE").ok())
        .or(config.profile.clone())
        .unwrap_or_else(|| profile::DEFAULT.into());
    let profile = profile::load(&home, &profile_name)?;
    let imported = if args.no_import || config.import_from_other_harnesses.as_deref() == Some("off")
    {
        Vec::new()
    } else {
        imports(&home, interactive, false)?
    };
    let sources = Sources {
        home: &home,
        project: project.as_deref(),
        allowed: profile.plugins.as_deref(),
        imported,
    };
    let library = sources.library();
    for shadowed in &library.shadowed {
        eprintln!("axio: ignored duplicate {shadowed}");
    }
    let spec = args
        .model
        .or_else(|| std::env::var("AXIO_MODEL").ok())
        .or(config.model)
        .or(profile.model.clone())
        .unwrap_or_else(|| provider::DEFAULT_MODEL.into());
    let mut agent = Agent::new(
        Model::parse(&spec),
        cwd.clone(),
        context::system_prompt(&cwd, library.prompt_section().as_deref()),
    );
    agent.effort = args
        .effort
        .clone()
        .or_else(|| std::env::var("AXIO_EFFORT").ok())
        .or(config.effort)
        .or(profile.effort.clone());
    agent.compact_ratio = profile.compact_at;

    let sessions = session::dir_for(&home, &cwd);
    let resume = match (args.session, args.resume_latest) {
        (Some(p), _) => Some(p),
        (None, true) => Some(session::latest(&sessions).ok_or("no session to continue here")?),
        _ => None,
    };
    let source = if resume.is_some() {
        "resume"
    } else {
        "startup"
    };
    if let Some(path) = resume {
        agent.messages = session::load(&path)?;
        // No reply has measured the context yet; estimate it so a long
        // resumed session can still compact before its first prompt.
        agent.context = compact::estimate_tokens(&agent.messages);
        agent.session = Some(session::Session::open(path)?);
    } else if !args.no_session {
        agent.session = Some(session::Session::create(&sessions, &cwd)?);
    }
    agent.hooks = sources.hooks(&cwd)?;
    for skipped in &agent.hooks.skipped {
        eprintln!("axio: skipped hook: {skipped}");
    }
    let servers = sources.mcp()?;
    if let Some(sub) = &args.mcp_command {
        mcp_command(sub, &servers)?;
        return Ok(ExitCode::SUCCESS);
    }
    for skipped in &servers.skipped {
        eprintln!("axio: skipped MCP server: {skipped}");
    }
    let started = mcp::Mcp::start(&servers, &cwd);
    for failed in &started.failed {
        eprintln!("axio: MCP server failed to start: {failed}");
    }
    if profile.modules.sandbox {
        let root = settings::project_root(&cwd);
        sandbox::activate(sandbox::Sandbox::for_project(
            &root,
            config.sandbox_network.unwrap_or(false),
        ))?;
    }
    agent.set_tools(profile.modules, started);

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
        agent.start(source, &mut term)?;
        let prompt = match command(&library, &prompt) {
            Some(Ok(expanded)) => expanded,
            Some(Err(e)) => return Err(e),
            None => prompt,
        };
        let result = agent.prompt(&prompt, &mut term);
        agent.end("other", &mut term);
        background::stop_all();
        result?;
        return Ok(ExitCode::SUCCESS);
    }

    let _raw = term::RawMode::enter().ok_or("could not configure the terminal (stty failed)")?;
    term.keys = Some(term::Keys::spawn());
    agent.start(source, &mut term)?;
    eprintln!(
        "{}",
        term.dim(&format!(
            "axio {} · {} · {} · Esc stops a turn · /quit to exit",
            env!("CARGO_PKG_VERSION"),
            agent.model,
            profile.name
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
            ("/compact", _) => match agent.compact("manual", &mut term) {
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
            ("/help", _) => eprintln!("{}", help(&library)),
            ("/profile", _) => eprintln!(
                "{} · modules: {} · plugins: {}",
                profile.name,
                match agent.modules.names().join(", ") {
                    n if n.is_empty() => "none".to_string(),
                    n => n,
                },
                profile
                    .plugins
                    .as_ref()
                    .map_or("all".to_string(), |p| p.join(", "))
            ),
            _ => {
                let text = match command(&library, input) {
                    Some(Ok(expanded)) => expanded,
                    Some(Err(e)) => {
                        eprintln!("axio: {e}");
                        continue;
                    }
                    None => input.to_string(),
                };
                term.turn = Usage::default();
                term.thinking = false;
                match agent.prompt(&text, &mut term) {
                    Ok(()) => {}
                    Err(e) if e == agent::ABORTED => eprintln!("\n{}", term.dim("[stopped]")),
                    Err(e) => eprintln!("\naxio: {e}"),
                }
                eprintln!("{}", term.dim(&status(&agent, &term.turn)));
            }
        }
    }
    agent.end("prompt_input_exit", &mut term);
    background::stop_all();
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_input_is_a_command_only_when_it_names_one() {
        let lib = skills::Library::default();
        assert_eq!(command(&lib, "fix the bug"), None);
        assert_eq!(
            command(&lib, "/usr/bin/env is broken"),
            None,
            "paths are text"
        );
        assert_eq!(
            command(&lib, "/nope a b"),
            Some(Err("unknown command /nope; /help lists them".into()))
        );
    }
}
