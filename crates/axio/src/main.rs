//! axio — a coding agent for the terminal.
//!
//! One binary, two surfaces. Which one runs is decided by how it was invoked,
//! not by a flag the caller has to remember.

mod render;
mod surface;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use axio_core::agent::Agent;
use axio_core::approver::NonInteractive;
use axio_core::config::{self, Flags, Paths, Resolved};
use axio_core::protocol::{Notice, SessionId, TurnOutcome};
use axio_core::provider::SystemBlock;
use axio_core::record::{
    self, Header, Recorder, SESSION_FORMAT_VERSION, SessionFile, SessionStore,
};
use axio_core::session::Session;
use axio_core::tool::ToolEnv;
use axio_provider::AnthropicProvider;
use axio_provider::client::load_api_key;
use clap::Parser;
use render::{JsonlRenderer, PlainRenderer, Renderer, Style};
use surface::Surface;
use tokio_util::sync::CancellationToken;

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("AXIO_BUILD_SHA"), ")");

#[derive(Parser, Debug)]
#[command(name = "axio", version = VERSION, about = "A coding agent for the terminal.")]
struct Cli {
    /// Run one turn with this prompt and exit. Piped stdin is appended.
    #[arg(short, long)]
    prompt: Option<String>,

    /// Emit the event stream as one JSON object per line. Unstable.
    #[arg(long)]
    json: bool,

    /// Approve every action without asking. Unattended and unsandboxed.
    #[arg(long)]
    yes: bool,

    /// Report configuration, credentials and assumed prices, then exit.
    #[arg(long)]
    doctor: bool,

    /// Continue a previous session. A unique prefix of its id is enough.
    #[arg(long, value_name = "ID", conflicts_with = "ephemeral")]
    resume: Option<String>,

    /// List recent sessions and exit.
    #[arg(long, conflicts_with_all = ["prompt", "resume"])]
    list: bool,

    /// Record nothing to disk.
    #[arg(long)]
    ephemeral: bool,

    /// Explain where a configuration key's value came from, then exit.
    #[arg(long, value_name = "KEY")]
    explain: Option<String>,

    /// Override the model for this run.
    #[arg(long, value_name = "NAME")]
    model: Option<String>,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the async runtime");

    std::process::ExitCode::from(runtime.block_on(run(cli)))
}

async fn run(cli: Cli) -> u8 {
    let resolved = resolve_config(&cli);

    // Local modes answer from configuration alone and must never touch stdin,
    // a credential or the network.
    if cli.doctor {
        return doctor(&resolved);
    }
    if let Some(key) = &cli.explain {
        return explain(&resolved, key);
    }
    if cli.list {
        return list_sessions();
    }

    // Announced before anything else, including the credential check. An
    // unattended, unsandboxed mode should never be silent about itself, and
    // whether it was enabled does not depend on whether the run gets any
    // further than this.
    if cli.yes {
        eprintln!(
            "axio: --yes is on. Every action is approved without asking, there is no sandbox, \
             and commands run with your permissions."
        );
    }

    let stdin_is_tty = std::io::stdin().is_terminal();
    let stdout_is_tty = std::io::stdout().is_terminal();

    // Read piped stdin before deciding, so `-p` can append it.
    let piped = read_stdin(stdin_is_tty, cli.prompt.is_some());

    match Surface::select(cli.prompt.as_deref(), piped.as_deref(), stdin_is_tty) {
        Surface::OneShot(prompt) => one_shot(&cli, &resolved, prompt, stdout_is_tty).await,
        Surface::Tui => {
            eprintln!(
                "axio: the interactive interface is not built yet.\n\
                 Run a single turn instead:  axio -p \"your prompt\"\n\
                 or pipe input:              echo \"your prompt\" | axio"
            );
            2
        }
        Surface::Nothing => {
            eprintln!(
                "axio: no prompt.\n\
                 Try:  axio -p \"your prompt\"\n\
                 or:   echo \"your prompt\" | axio"
            );
            2
        }
    }
}

async fn one_shot(cli: &Cli, resolved: &Resolved, prompt: String, stdout_is_tty: bool) -> u8 {
    let api_key = match load_api_key() {
        Ok(key) => key,
        Err(e) => {
            eprintln!(
                "axio: {e}\n\n\
                 Set a key for this shell:\n    export ANTHROPIC_API_KEY=sk-ant-...\n\n\
                 Then re-run the same command. `axio --doctor` shows what axio can currently see."
            );
            return 1;
        }
    };

    let provider = match AnthropicProvider::new(api_key) {
        Ok(p) => Arc::new(p),
        Err(e) => {
            eprintln!("axio: could not start the http client: {e}");
            return 1;
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut cfg = resolved.runtime();
    cfg.spill_dir = Some(state_dir().join("outputs"));

    let mut notices: Vec<Notice> = resolved.notices().to_vec();
    let (policy, mut policy_notices) = resolved.policy(cli.yes);
    notices.append(&mut policy_notices);

    // Resume before the agent exists: the header decides the model, because a
    // transcript's reasoning is only replayable under the model that minted it.
    let store = SessionStore::new(state_dir().join("sessions"));
    let (mut session, resumed, mut recorder) = match &cli.resume {
        Some(needle) => match open_resumed(&store, needle, cli.model.as_deref(), &mut notices) {
            Ok(parts) => parts,
            Err(message) => {
                eprintln!("axio: {message}");
                return 1;
            }
        },
        None => {
            let session = Session::new(cwd.clone(), &cfg.model);
            let recorder = new_recorder(cli, &store, &session, &prompt, &mut notices);
            (session, false, recorder)
        }
    };
    if resumed {
        cfg = cfg.adopt_model(session.model());
        recorder.append(&record::Record::Resumed {
            at_ms: now_ms(),
            model: session.model().to_owned(),
        });
    }
    session.take_files_changed();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let mut agent = Agent::new(
        provider,
        Arc::new(if cli.yes {
            NonInteractive::allow()
        } else {
            NonInteractive::deny()
        }),
        session,
        cfg.clone(),
        vec![SystemBlock {
            text: system_prompt(&cwd),
        }],
        tx,
    )
    .with_policy(policy)
    .with_recorder(recorder)
    .with_env(ToolEnv {
        vars: axio_tools::proc::child_env(),
    });

    for tool in axio_tools::all() {
        agent.register_tool(tool);
    }

    // Colour is a property of the sink, not of the session: `axio -p x >
    // out.txt` run from a terminal must still write zero escape bytes.
    let style = if stdout_is_tty && std::env::var_os("NO_COLOR").is_none() {
        Style { colour: true }
    } else {
        Style::PLAIN
    };
    let mut renderer: Box<dyn Renderer + Send> = if cli.json {
        Box::new(JsonlRenderer::new(std::io::stdout()))
    } else {
        Box::new(PlainRenderer::new(
            std::io::stdout(),
            Box::new(std::io::stderr()),
            style,
        ))
    };

    let cancel = CancellationToken::new();
    let signal_code = surface::spawn_signal_watcher(cancel.clone());

    agent.announce(resumed, notices);

    // The renderer drains on this task while the loop runs on another, so a
    // slow sink cannot stall the turn and a long turn cannot starve the sink.
    let turn = tokio::spawn(async move {
        let outcome = agent.run_turn(prompt, cancel).await;
        (outcome, agent)
    });

    let mut pending = Some(turn);
    let outcome = loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                if let Err(e) = renderer.handle(&event)
                    && e.kind() != std::io::ErrorKind::BrokenPipe
                {
                    // A closed pipe (`axio -p x | head`) is not worth shouting about.
                    eprintln!("axio: render error: {e}");
                }
            }
            result = async { pending.as_mut().expect("guarded by the condition").await },
                     if pending.is_some() =>
            {
                match result {
                    Ok((outcome, _agent)) => {
                        // Drain what is still queued before reporting.
                        while let Ok(event) = rx.try_recv() {
                            let _ = renderer.handle(&event);
                        }
                        break outcome;
                    }
                    Err(e) => {
                        break TurnOutcome::Failed { message: format!("the turn panicked: {e}") };
                    }
                }
            }
            else => break TurnOutcome::Failed {
                message: "event stream closed unexpectedly".into(),
            },
        }
    };

    let _ = renderer.finish();

    // A signal's own exit code outranks the outcome's: the caller asked the
    // process to stop, and the shell reports how it stopped.
    match signal_code.load(std::sync::atomic::Ordering::SeqCst) {
        0 => outcome.exit_code(),
        code => code,
    }
}

// ---------------------------------------------------------------- local modes

fn resolve_config(cli: &Cli) -> Resolved {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let user = user_config_path();
    // Bounded at the home directory so the walk cannot reach into an unrelated
    // parent and apply someone else's project settings.
    let project = config::find_project_config(&cwd, home_dir().as_deref());
    let env: Vec<(String, String)> = std::env::vars().collect();
    config::resolve(
        &Paths { user, project },
        &env,
        &Flags {
            model: cli.model.clone(),
            effort: None,
        },
    )
}

fn home_dir() -> Option<PathBuf> {
    use etcetera::BaseStrategy;
    etcetera::choose_base_strategy()
        .ok()
        .map(|s| s.home_dir().to_path_buf())
}

fn user_config_path() -> Option<PathBuf> {
    use etcetera::BaseStrategy;
    etcetera::choose_base_strategy()
        .ok()
        .map(|s| s.config_dir().join("axio").join("config.toml"))
}

fn explain(resolved: &Resolved, key: &str) -> u8 {
    match resolved.explain(key) {
        Some(layer) => {
            println!("{key} came from {}", layer.describe());
            0
        }
        None => {
            eprintln!("axio: no such configuration key: {key}");
            eprintln!("known keys:");
            for k in resolved.keys() {
                eprintln!("  {k}");
            }
            2
        }
    }
}

fn list_sessions() -> u8 {
    let store = SessionStore::new(state_dir().join("sessions"));
    let files = store.files();
    if files.is_empty() {
        println!("no sessions yet");
        return 0;
    }
    // One line read per file: a header carries everything a listing shows, so
    // listing never parses a transcript.
    for path in files.iter().take(50) {
        match record::read_header(path) {
            Ok(h) => {
                let label = h.label.as_deref().unwrap_or("");
                println!(
                    "{}  {}  {}  {}",
                    &h.id.to_string()[..8],
                    h.started,
                    h.model,
                    label
                );
            }
            Err(_) => println!("{}  (unreadable)", path.display()),
        }
    }
    if files.len() > 50 {
        println!("… and {} more", files.len() - 50);
    }
    0
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Start recording a new session, unless asked not to.
fn new_recorder(
    cli: &Cli,
    store: &SessionStore,
    session: &Session,
    prompt: &str,
    notices: &mut Vec<Notice>,
) -> Recorder {
    if cli.ephemeral {
        return Recorder::Ephemeral;
    }
    let header = Header {
        version: SESSION_FORMAT_VERSION,
        protocol: axio_core::PROTOCOL_VERSION,
        id: session.id(),
        cwd: session.cwd().clone(),
        model: session.model().to_owned(),
        started: iso_now(),
        label: Some(label_from(prompt)),
        axio: env!("CARGO_PKG_VERSION").to_owned(),
    };
    match SessionFile::create(store.path_for(session.id()), &header) {
        Ok(file) => Recorder::File(file),
        Err(e) => {
            // Losing the log is not a reason to refuse to work.
            notices.push(Notice::warn(format!(
                "not recording this session ({e}); it will not be resumable"
            )));
            Recorder::Ephemeral
        }
    }
}

/// Reopen a session so the turn continues the same file.
type ResumedParts = (Session, bool, Recorder);

fn open_resumed(
    store: &SessionStore,
    needle: &str,
    model_override: Option<&str>,
    notices: &mut Vec<Notice>,
) -> Result<ResumedParts, String> {
    let id: SessionId = store.resolve(needle)?;
    let path = store.path_for(id);
    let loaded = record::load(&path).map_err(|e| format!("cannot resume {id}: {e}"))?;

    let mut session = loaded.session;
    notices.extend(loaded.notices);
    if loaded.degraded {
        notices.push(Notice::warn(
            "part of this session could not be read; the model is seeing a history with a hole in it",
        ));
    }
    if let Some(model) = model_override
        && model != session.model()
    {
        notices.push(Notice::warn(format!(
            "resuming under {model} instead of {}; earlier reasoning will not be replayed",
            session.model()
        )));
        session.adopt_model(model);
    }

    let recorder = match SessionFile::reopen(path) {
        Ok(file) => Recorder::File(file),
        Err(e) => {
            notices.push(Notice::warn(format!("continuing without recording ({e})")));
            Recorder::Ephemeral
        }
    };
    Ok((session, true, recorder))
}

fn iso_now() -> String {
    // Seconds since the epoch is not a date a human reads, but pulling a date
    // library into the binary for one string is not worth it either; the ULID
    // carries the authoritative timestamp.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// One line of the first prompt, so a listing is readable.
fn label_from(prompt: &str) -> String {
    let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let cleaned: String = line.chars().filter(|c| !c.is_control()).take(80).collect();
    cleaned.trim().to_owned()
}

/// How long to wait for supplementary stdin before giving up on it.
///
/// Only applies when `-p` was given, so stdin is optional. Override with
/// `AXIO_STDIN_WAIT_MS` if a slow producer ever needs longer.
fn stdin_wait() -> std::time::Duration {
    let ms = std::env::var("AXIO_STDIN_WAIT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(500);
    std::time::Duration::from_millis(ms)
}

/// Read stdin, without ever hanging forever on a pipe that has nothing to say.
///
/// The distinction is what stdin *means* for this invocation:
///
/// * No `-p`: stdin **is** the prompt (`echo hi | axio`). Blocking until EOF is
///   correct — there is nothing else to run.
/// * With `-p`: stdin is supplementary (`cat f | axio -p "review this"`). An
///   inherited-but-idle stdin — a supervisor, a background job, a harness that
///   holds the pipe open and never writes — would otherwise block the process
///   forever with no output at all. So it is read on a side thread and given a
///   bounded wait.
fn read_stdin(stdin_is_tty: bool, have_prompt: bool) -> Option<String> {
    if stdin_is_tty {
        return None;
    }

    if !have_prompt {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).ok();
        return Some(buf);
    }

    let (tx, rx) = std::sync::mpsc::channel();
    // Detached deliberately: if it is still blocked at exit the process is
    // going away anyway, and there is no portable way to cancel a blocking
    // read on stdin.
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    // A timeout means nothing was coming; proceed on the prompt alone.
    rx.recv_timeout(stdin_wait()).ok()
}

/// Where axio keeps state that is not the user's to curate.
fn state_dir() -> PathBuf {
    if let Some(explicit) = std::env::var_os("AXIO_STATE") {
        return PathBuf::from(explicit);
    }
    std::env::temp_dir().join(format!("axio-{}", std::process::id()))
}

fn system_prompt(cwd: &std::path::Path) -> String {
    format!(
        "You are axio, a coding agent running in a terminal.\n\
         Working directory: {}\n\
         Platform: {}\n\n\
         You have tools: read, write, edit, glob, grep and bash. Prefer the project's own \
         commands over reimplementing what they do.\n\n\
         Keep responses focused, brief, and concise. Lead with the outcome, then the detail.\n\
         Deliver what was asked at the scope intended: make routine judgement calls yourself, \
         and check in only when different readings would lead to materially different work.\n\
         You are operating in a single turn; the user cannot answer questions mid-task.",
        cwd.display(),
        std::env::consts::OS,
    )
}

/// What axio can see, so a misconfiguration is one command away from obvious.
fn doctor(resolved: &Resolved) -> u8 {
    let mut out = std::io::stdout();
    let cfg = resolved.config();
    let _ = writeln!(out, "axio {VERSION}");
    let _ = writeln!(out);

    let _ = writeln!(out, "credentials");
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) if !k.trim().is_empty() => {
            let _ = writeln!(
                out,
                "  ANTHROPIC_API_KEY   set ({} chars, never printed)",
                k.len()
            );
        }
        _ => {
            let _ = writeln!(out, "  ANTHROPIC_API_KEY   not set");
            let _ = writeln!(out, "  -> export ANTHROPIC_API_KEY=sk-ant-...");
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "model");
    let _ = writeln!(out, "  model               {}", cfg.model.name);
    let _ = writeln!(out, "  effort              {}", cfg.model.effort.as_wire());
    let _ = writeln!(out, "  max_tokens          {}", cfg.model.max_tokens);
    let _ = writeln!(out, "  max_steps           {}", cfg.budget.max_steps);
    let _ = writeln!(out);

    // Printed because a stale price table is otherwise invisible until the bill.
    let _ = writeln!(out, "assumed prices (USD per million tokens)");
    let _ = writeln!(out, "  input               5.00");
    let _ = writeln!(out, "  output              25.00");
    let _ = writeln!(out, "  cache read          0.50");
    let _ = writeln!(out, "  cache write         6.25");
    let _ = writeln!(out);

    let _ = writeln!(out, "paths");
    let _ = writeln!(
        out,
        "  user config         {}",
        user_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unknown)".into())
    );
    let _ = writeln!(out, "  state               {}", state_dir().display());
    let _ = writeln!(
        out,
        "  cwd                 {}",
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "surfaces");
    let _ = writeln!(
        out,
        "  stdin               {}",
        if std::io::stdin().is_terminal() {
            "terminal"
        } else {
            "piped"
        }
    );
    let _ = writeln!(
        out,
        "  stdout              {}",
        if std::io::stdout().is_terminal() {
            "terminal"
        } else {
            "piped"
        }
    );
    0
}
