mod agent;
mod auth;
mod background;
mod chatgpt;
mod child;
mod compact;
mod context;
mod crypto;
mod editor;
mod hooks;
mod import;
mod interactive;
mod markdown;
mod mcp;
mod menu;
mod message;
mod oauth;
mod plan;
mod plugin_cli;
mod plugins;
mod profile;
mod provider;
mod roles;
mod sandbox;
mod search;
mod session;
mod settings;
mod skills;
mod sse;
mod subagent;
mod term;
mod toml_lite;
mod tools;
mod ui;
mod web;

use agent::Agent;
use provider::Model;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "axio: a minimal coding agent

Usage: axio [options] [prompt...]
       axio plugin install|list|update|remove ...
       axio import   (use hooks, plugins and MCP servers from other harnesses)
       axio mcp list | login <name> | logout <name>
       axio auth list | set <provider> [field] | remove <provider>
       axio profile [name]   (show or set the default profile)

ax is the same as axio; axio-full, axio-min (or any axio-<profile>) start
that profile.

Options:
  -p, --print           Answer one prompt (argument or stdin) and exit
  -m, --model <model>   provider/model, e.g. anthropic/claude-opus-5-5,
                        openai/<id>, openrouter/<vendor>/<id> (env AXIO_MODEL)
  -e, --effort <level>  low, medium, high, xhigh or max (env AXIO_EFFORT)
  -c, --continue        Continue the most recent session in this directory
  -r, --resume          Pick one of this directory's recent sessions
  -s, --session <file>  Continue a session file
      --no-session      Do not save this session
      --profile <name>  minimal, standard (default), full or one in
                        ~/.axio/agent/profiles/ (env AXIO_PROFILE)
      --trust           Trust this project's configuration for this run
      --no-project-config  Ignore this project's .axio/ and .mcp.json
      --no-import       Skip hooks, plugins and servers from other harnesses
  -h, --help            Show this help
  -V, --version         Show the version

Interactive: Enter sends, Alt-Enter or a trailing \\ adds a line, Tab
completes a /command or an @path, Up/Down walk history, Ctrl-R searches it,
Esc or Ctrl-C stops a running turn, Enter during one sends what you typed to
the model at its next step, Ctrl-D exits. @path attaches a file.
Commands: /new, /undo, /output, /compact, /model <model>, /effort <level>,
/session, /help, /quit, plus your own

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
    pick_session: bool,
    session: Option<PathBuf>,
    no_session: bool,
    trust: bool,
    profile: Option<String>,
    no_project_config: bool,
    no_import: bool,
    /// `axio mcp …`, run after configuration loads.
    mcp_command: Option<Vec<String>>,
    /// `axio agents`: list the sub-agent roles.
    list_roles: bool,
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
            "-r" | "--resume" => a.pick_session = true,
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

/// Lists this directory's recent sessions and reads which to continue.
fn pick_session(dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let recent = session::recent(dir, 10);
    if recent.is_empty() {
        return Err("no saved sessions in this directory".into());
    }
    let now = std::time::SystemTime::now();
    for (i, s) in recent.iter().enumerate() {
        let secs = now.duration_since(s.modified).map_or(0, |d| d.as_secs());
        let age = match secs {
            0..60 => "just now".to_string(),
            60..3600 => format!("{}m ago", secs / 60),
            3600..86_400 => format!("{}h ago", secs / 3600),
            _ => format!("{}d ago", secs / 86_400),
        };
        let first: String = s
            .first_prompt
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(70)
            .collect();
        eprintln!(
            "{:>2}  {age:>8}  {first}  \x1b[2m({} messages)\x1b[0m",
            i + 1,
            s.messages
        );
    }
    eprint!("Continue which? [1] ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let n = match line.trim() {
        "" => 1,
        t => t
            .parse::<usize>()
            .map_err(|_| format!("not a number: {t}"))?,
    };
    recent
        .into_iter()
        .nth(n.wrapping_sub(1))
        .map(|s| s.path)
        .ok_or_else(|| format!("there is no session {n}"))
}

/// The model with its effort, for the header and /model.
fn model_line(agent: &Agent) -> String {
    let mut s = agent.model.id.clone();
    if let Some(e) = &agent.effort {
        s.push_str(&format!(" {e}"));
    }
    s
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
        _ if args.trust => true,
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
        // `axio import` already listed what an imported item cannot do.
        let noticed = h.skipped.len();
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
        h.skipped.truncate(noticed);
        Ok(h)
    }

    /// Role files: plugins' (global, imported, then the project's), then
    /// `~/.axio/agent/agents`, then a trusted project's `.axio/agents`.
    fn roles(&self) -> (Vec<roles::Role>, Vec<String>) {
        let mut plugin_roots: Vec<std::path::PathBuf> = self
            .plugins(self.home.join("plugins"))
            .into_iter()
            .map(|p| p.root)
            .collect();
        plugin_roots.extend(
            self.imported_plugins()
                .map(|(_, root, _)| root.to_path_buf()),
        );
        if let Some(root) = self.project {
            plugin_roots.extend(
                self.plugins(root.join(".axio/plugins"))
                    .into_iter()
                    .map(|p| p.root),
            );
        }
        let mut dirs = roles::dirs(self.home, self.project, &plugin_roots);
        // Imported folders sit just below axio's own.
        let at = plugin_roots.len();
        for i in &self.imported {
            if let import::Kind::Agents(dir) = &i.kind {
                dirs.insert(at, dir.clone());
            }
        }
        roles::load(&dirs)
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
        let noticed = c.skipped.len();
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
        c.skipped.truncate(noticed);
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
        // The same skill installed in several harnesses is expected.
        let noticed = lib.shadowed.len();
        for i in &self.imported {
            if let import::Kind::Skills(base) = &i.kind {
                lib.add_dir(base, None);
            }
        }
        for (name, root, _) in self.imported_plugins() {
            lib.add_dir(root, Some(name));
        }
        lib.shadowed.truncate(noticed);
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

/// Built-in commands: name, arguments and what each does.
const BUILTINS: [(&str, &str, &str); 16] = [
    ("model", "[provider/model]", "pick or switch the model"),
    ("login", "[provider]", "save an API key for a provider"),
    ("logout", "[provider]", "remove a provider's saved key"),
    ("effort", "[level]", "pick or set reasoning effort"),
    ("new", "", "start a new session"),
    ("resume", "", "pick a saved session to continue"),
    (
        "undo",
        "",
        "put back the files the last turn wrote or edited",
    ),
    ("output", "", "show the last tool call's full result"),
    ("compact", "", "summarize the conversation now"),
    ("profile", "", "show the profile, modules and plugins"),
    ("session", "", "show the session file"),
    ("help", "", "list the commands"),
    ("quit", "", "exit (or Ctrl-D)"),
    ("exit", "", "exit (or Ctrl-D)"),
    ("clear", "", "start a new session"),
    ("models", "", "pick the model"),
];

/// Other names for built-ins, which work but stay out of the menu and help.
const ALIASES: [&str; 3] = ["exit", "clear", "models"];

/// Every slash command with what it does, built-ins first.
fn command_list(lib: &skills::Library) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = BUILTINS
        .iter()
        .filter(|(n, _, _)| !ALIASES.contains(n))
        .map(|(n, _, about)| (n.to_string(), about.to_string()))
        .collect();
    for c in &lib.commands {
        rows.push((c.name.clone(), c.description.clone()));
    }
    for k in lib.skills.iter().filter(|k| k.invocable) {
        let about: String = k.description.chars().take(70).collect();
        rows.push((k.name.clone(), format!("{about} (skill)")));
    }
    rows
}

fn help(lib: &skills::Library) -> String {
    let mut rows: Vec<(String, String)> = BUILTINS
        .iter()
        .filter(|(n, _, _)| !ALIASES.contains(n))
        .map(|(n, args, about)| {
            let usage = if args.is_empty() {
                format!("/{n}")
            } else {
                format!("/{n} {args}")
            };
            (usage, about.to_string())
        })
        .collect();
    for c in &lib.commands {
        rows.push((format!("/{}", c.name), c.description.clone()));
    }
    for k in lib.skills.iter().filter(|k| k.invocable) {
        let about: String = k.description.chars().take(70).collect();
        rows.push((format!("/{}", k.name), format!("{about} (skill)")));
    }
    let width = rows
        .iter()
        .map(|(a, _)| a.chars().count())
        .max()
        .unwrap_or(0);
    let mut s: String = rows
        .iter()
        .map(|(a, b)| format!("  {a:width$}  {b}\n"))
        .collect();
    s.push_str("\n  / opens the command menu · Enter sends · Alt-Enter or a trailing \\ adds a line · Up/Down history · Esc stops a turn");
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
        let home = context::home();
        let accepted = imports(&home, true, true)?;
        // Scanning is off until the user opts in; accepting something
        // turns it on so those items load and new ones are offered.
        if !accepted.is_empty() {
            settings::set(&home, "importFromOtherHarnesses", serde_json::json!("ask"))?;
        }
        println!(
            "{} item(s) from other harnesses in use{}",
            accepted.len(),
            if accepted.is_empty() {
                ""
            } else {
                "; axio now checks for new ones at start"
            }
        );
        return Ok(ExitCode::SUCCESS);
    }
    // Hidden: a sub-agent's process, started by a parent axio.
    if all.first().is_some_and(|a| a == "agent") {
        return Ok(if child::main(&all[1..])? {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
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
        let home = context::home();
        let importing = settings::Settings::load(&home, None)?
            .import_from_other_harnesses
            .is_some_and(|v| v == "ask");
        let imported = if importing {
            imports(&home, false, false)?
        } else {
            Vec::new()
        };
        plugin_cli::main(&all[1..], &home, &cwd, imported)?;
        return Ok(ExitCode::SUCCESS);
    }
    let args = if all.first().is_some_and(|a| a == "agents") {
        Args {
            list_roles: true,
            print: true,
            no_session: true,
            ..Args::default()
        }
    } else if all.first().is_some_and(|a| a == "mcp") {
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
    // Off unless the user opted in with `axio import` or the setting.
    let imported = if args.no_import || config.import_from_other_harnesses.as_deref() != Some("ask")
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
    agent.compact_model = config.compact_model.clone();
    agent.fast_model = config.subagents.fast_model.clone();

    let sessions = session::dir_for(&home, &cwd);
    let resume = match (args.session, args.resume_latest, args.pick_session) {
        (Some(p), _, _) => Some(p),
        (None, true, _) => Some(session::latest(&sessions).ok_or("no session to continue here")?),
        (None, false, true) => Some(pick_session(&sessions)?),
        _ => None,
    };
    let source = if resume.is_some() {
        "resume"
    } else {
        "startup"
    };
    let mut resumed = 0;
    if let Some(path) = resume {
        agent.messages = session::load(&path)?;
        resumed = agent.messages.len();
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
    let (roles, role_errors) = sources.roles();
    for e in &role_errors {
        eprintln!("axio: skipped role file {e}");
    }
    if args.list_roles {
        for r in &roles {
            let pick = |p: &roles::Pick| match p {
                roles::Pick::Inherit => "inherit".to_string(),
                roles::Pick::Choose => "choose".to_string(),
                roles::Pick::Value(v) => v.clone(),
            };
            let tools = r
                .tools
                .as_ref()
                .map_or("the main agent's".into(), |t| t.join(", "));
            println!(
                "{}  model {} · effort {} · tools {tools}\n    {}\n    {}",
                r.name,
                pick(&r.model),
                pick(&r.effort),
                r.description,
                r.source
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    if profile.modules.agents && subagent::depth() == 0 {
        agent.set_subagents(subagent::Host::new(roles, config.subagents.clone(), 0)?);
    }
    if !agent.messages.is_empty() {
        // No reply has measured the context yet; estimate it so a long
        // resumed session can still compact before its first prompt.
        agent.context = agent.estimated_context();
    }

    let no_color = std::env::var_os("NO_COLOR").is_some();
    let mut term = ui::Term::new(
        &cwd,
        io::stderr().is_terminal() && !no_color,
        io::stdout().is_terminal() && !no_color,
    );

    let mut prompt = args.prompt.join(" ");
    if args.print || !io::stdin().is_terminal() {
        // With a prompt argument, stdin is extra input (`git diff | axio -p
        // "review"`); wait a little for it rather than forever.
        let has_input = prompt.is_empty() || term::input_within(Duration::from_secs(3));
        if !has_input {
            eprintln!("axio: nothing arrived on stdin within 3s; going on without it");
        }
        if has_input && (prompt.is_empty() || !io::stdin().is_terminal()) {
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

    if agent.model.provider == chatgpt::PROVIDER {
        if auth::missing_key(chatgpt::PROVIDER) {
            eprintln!("axio: not signed in to ChatGPT yet; run /login chatgpt");
        }
    } else if auth::missing_key(&agent.model.provider) {
        auth::first_run(&agent.model.provider, &home)?;
    }
    let _raw = term::RawMode::enter().ok_or("could not configure the terminal (stty failed)")?;
    term.keys = Some(term::Keys::spawn());
    agent.start(source, &mut term)?;
    eprintln!(
        "{} {}",
        term.paint("\x1b[1m", "axio"),
        term.dim(&format!(
            "{} · {} · {}",
            model_line(&agent),
            profile.name,
            ui::short_path(
                &cwd.display().to_string(),
                std::path::Path::new("/nonexistent")
            )
        ))
    );
    let resumed = match resumed {
        0 => String::new(),
        n => format!("resumed {n} message{} · ", if n == 1 { "" } else { "s" }),
    };
    eprintln!(
        "{}",
        term.dim(&format!("{resumed}Esc stops a turn · /help · Ctrl-D exits"))
    );
    let recap = ui::recap(&agent.messages);
    if !recap.is_empty() {
        eprintln!();
        for line in recap {
            eprintln!("{}", term.dim(&line));
        }
    }
    let mut editor = editor::Editor::default();
    editor.history = load_history();
    editor.cwd = Some(cwd.clone());
    editor.commands = command_list(&library);
    let mut models = interactive::ModelCache::new();
    editor.color = term.color;
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
            ("/model" | "/models", "") => {
                interactive::pick_model(&mut agent, &mut term, &mut models, None);
            }
            ("/login", p) => interactive::login(&mut agent, &mut term, &mut models, p),
            ("/logout", p) => interactive::logout(&mut term, &mut models, p),
            ("/effort", "") => interactive::pick_effort(&mut agent, &mut term),
            ("/resume", _) => interactive::resume(&mut agent, &mut term, &sessions),
            ("/new" | "/clear", _) => {
                agent.messages.clear();
                agent.context = 0;
                if agent.session.is_some() {
                    agent.session = Some(session::Session::create(&sessions, &cwd)?);
                }
                eprintln!("{}", term.dim("new session"));
            }
            ("/compact", _) => match agent.compact("manual", &mut term) {
                Ok(true) => {}
                Ok(false) => eprintln!("{}", term.dim("nothing older to compact")),
                Err(e) => eprintln!("axio: {e}"),
            },
            ("/output", _) => match &term.last_output {
                Some((title, text)) => {
                    eprintln!("{}", term.dim(&format!("── {title}")));
                    println!("{}", text.trim_end());
                }
                None => eprintln!("{}", term.dim("no tool has run yet")),
            },
            ("/undo", _) => match agent.undo() {
                Ok(done) => {
                    for (path, restored) in done {
                        let what = if restored { "restored" } else { "removed" };
                        let path = ui::short_path(&path.display().to_string(), &cwd);
                        eprintln!("{}", term.dim(&format!("{what} {path}")));
                    }
                }
                Err(e) => eprintln!("{}", term.dim(&e)),
            },
            ("/effort", level) => {
                agent.effort = Some(level.to_string());
                eprintln!("{}", term.dim(&format!("effort: {level} for this session")));
            }
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
                term.begin_turn();
                match agent.prompt(&text, &mut term) {
                    Ok(()) => {}
                    Err(e) if e == agent::ABORTED => {
                        agent::Ui::notice(&mut term, "stopped");
                    }
                    Err(e) => {
                        agent::Ui::notice(&mut term, "");
                        eprintln!("{}", term.paint("\x1b[31m", &format!("axio: {e}")));
                    }
                }
                eprintln!(
                    "{}",
                    term.status(agent.context, agent.model.context_window())
                );
                let left = term.take_queued();
                if !left.is_empty() {
                    prompt = left.join("\n");
                    println!();
                    println!("{}{prompt}", editor::PROMPT);
                }
            }
        }
    }
    agent.end("prompt_input_exit", &mut term);
    background::stop_all();
    eprintln!(
        "{}",
        term.farewell(agent.session.as_ref().map(|s| s.path.as_path()))
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_items_load_without_repeating_import_notices() {
        let root = std::env::temp_dir().join(format!("axio-quiet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |p: &str, t: &str| {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        write(
            "home/mcp.json",
            r#"{"mcpServers": {"browse": {"command": "browse"}}}"#,
        );
        write("home/skills/pdf/SKILL.md", "---\nname: pdf\n---\nmine");
        write(
            "codex/hooks.json",
            r#"{"hooks": {"Notification": [{"hooks": [{"type": "command", "command": "true"}]}],
                "Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
        );
        write("pi/skills/pdf/SKILL.md", "---\nname: pdf\n---\ntheirs");
        let item = |kind, label: &'static str| import::Item {
            harness: "Codex",
            kind,
            label,
            name: if label == "mcp" { "browse" } else { "x" }.into(),
            version: None,
            support: import::Support::Works,
            fingerprint: String::new(),
        };
        let sources = Sources {
            home: &root.join("home"),
            project: None,
            allowed: None,
            imported: vec![
                item(import::Kind::Hooks(root.join("codex/hooks.json")), "hooks"),
                item(
                    import::Kind::Mcp(serde_json::json!({"command": "other"})),
                    "mcp",
                ),
                item(import::Kind::Skills(root.join("pi")), "skills"),
            ],
        };
        let hooks = sources.hooks(&root).unwrap();
        assert!(hooks.skipped.is_empty(), "{:?}", hooks.skipped);
        assert!(hooks.has(hooks::Event::Stop), "the Stop hook still loads");
        let mcp = sources.mcp().unwrap();
        assert!(mcp.skipped.is_empty(), "{:?}", mcp.skipped);
        assert_eq!(mcp.servers.len(), 1);
        assert!(sources.library().shadowed.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trust_flag_decides_one_run_only() {
        let root = std::env::temp_dir().join(format!("axio-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, project) = (root.join("home"), root.join("project"));
        std::fs::create_dir_all(project.join(".axio")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(project.join(".axio/settings.json"), "{}").unwrap();
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let trusting = Args {
            trust: true,
            ..Args::default()
        };
        assert!(
            trusted_project(&home, &project, &trusting, false)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            settings::Trust::load(&home).decision(&project.canonicalize().unwrap()),
            settings::Decision::Unknown,
            "--trust is not remembered"
        );
        assert!(
            trusted_project(&home, &project, &Args::default(), false)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

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
