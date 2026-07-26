//! axio — a coding agent for the terminal.
//!
//! One binary, two surfaces. Which one runs is decided by how it was invoked,
//! not by a flag the caller has to remember.

mod render;
mod sandbox;
mod surface;
#[cfg(feature = "tui")]
mod tui;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use axio_core::agent::Agent;
use axio_core::approver::NonInteractive;
use axio_core::auth::{self, Secret};
use axio_core::config::{self, Flags, Paths, Resolved};
use axio_core::protocol::{Notice, SessionId, TurnOutcome};
use axio_core::provider::SystemBlock;
use axio_core::record::{
    self, Header, Recorder, SESSION_FORMAT_VERSION, SessionFile, SessionStore,
};
use axio_core::session::Session;
use axio_core::tool::ToolEnv;
use axio_provider::{AnthropicProvider, OLLAMA_BASE, OpenAiProvider};
use clap::{Parser, Subcommand};
use render::{JsonlRenderer, PlainRenderer, Refusals, Renderer, Style};
use surface::Surface;
use tokio_util::sync::CancellationToken;

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("AXIO_BUILD_SHA"), ")");

#[derive(Subcommand, Debug)]
enum Command {
    /// Manage stored credentials.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
}

#[derive(Subcommand, Debug)]
enum AuthAction {
    /// Store a credential, read from stdin.
    Login {
        /// Which provider the credential is for.
        #[arg(long, default_value = "anthropic")]
        provider: String,
    },
    /// Show which providers have a credential, and where it came from.
    Status,
    /// Remove a stored credential.
    Logout {
        #[arg(long, default_value = "anthropic")]
        provider: String,
    },
}

#[derive(Parser, Debug)]
#[command(name = "axio", version = VERSION, about = "A coding agent for the terminal.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

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

    /// Confine every command to the workspace, using the kernel's own sandbox.
    /// Linux only. Also settable as `[sandbox] enabled`.
    #[arg(long)]
    sandbox: bool,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    // Before the runtime, and that is not a preference. A Landlock domain
    // belongs to the calling *thread* and is inherited by threads it creates,
    // so restricting a tokio worker restricts that worker alone — and the
    // command runs on a different one. Applied here, every worker inherits it.
    let sandbox_notice = confine(&cli);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the async runtime");

    std::process::ExitCode::from(runtime.block_on(run(cli, sandbox_notice)))
}

/// Apply the sandbox if this invocation asked for one and will run a turn.
///
/// `auth login` writes to axio's own directory and the report-and-exit modes
/// touch nothing a command could, so neither is worth confining — and
/// confining the first would break it.
fn confine(cli: &Cli) -> Option<Notice> {
    let resolved = resolve_config(cli);
    let runs_a_turn = cli.command.is_none() && !cli.doctor && cli.explain.is_none() && !cli.list;
    if !runs_a_turn || !sandbox_requested(cli, &resolved) {
        return None;
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let plan = sandbox::Plan::new(&cwd, &state_dir(), &axio_home(), home_dir().as_deref())
        .allow_read(resolved.config().sandbox.read.iter().map(PathBuf::from))
        .allow_write(resolved.config().sandbox.write.iter().map(PathBuf::from));

    Some(match sandbox::apply(&plan) {
        sandbox::Outcome::Enforced => Notice::info(format!(
            "sandbox on: commands may write only under {} and read only the system",
            cwd.display()
        )),
        sandbox::Outcome::Partial(why) => Notice::warn(format!(
            "sandbox is only partly enforced ({why}); treat it as no confinement"
        )),
        sandbox::Outcome::Unsupported(why) => Notice::warn(format!(
            "sandbox requested but unavailable ({why}); nothing is confined"
        )),
    })
}

async fn run(cli: Cli, sandbox_notice: Option<Notice>) -> u8 {
    let mut resolved = resolve_config(&cli);
    if let Some(notice) = sandbox_notice {
        resolved.push_notice(notice);
    }

    if let Some(Command::Auth { action }) = &cli.command {
        return auth_command(action);
    }

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
        Surface::Tui => interactive(&cli, &resolved).await,
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

fn sandbox_requested(cli: &Cli, resolved: &Resolved) -> bool {
    cli.sandbox || resolved.config().sandbox.enabled
}

/// Everything a surface needs, built once so the two of them cannot drift.
///
/// The interactive path and the one-shot path resolve the same configuration,
/// protect the same directories and register the same tools. Two copies of this
/// is how one surface quietly ends up with a permission rule the other does not
/// apply.
struct Prepared {
    agent: Agent,
    events: tokio::sync::mpsc::UnboundedReceiver<axio_core::protocol::Event>,
    notices: Vec<Notice>,
    resumed: bool,
    /// Shown in the interactive banner. The headless build has no banner, and
    /// CI compiles it with `-D warnings`.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    model: String,
}

fn prepare(
    cli: &Cli,
    resolved: &Resolved,
    label: Option<&str>,
    approver: Arc<dyn axio_core::approver::Approver>,
) -> Result<Prepared, String> {
    let provider: Arc<dyn axio_core::provider::Provider> = build_provider(resolved)?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut cfg = resolved.runtime();
    cfg.spill_dir = Some(state_dir().join("outputs"));

    let mut notices: Vec<Notice> = resolved.notices().to_vec();
    let (policy, mut policy_notices) = resolved.policy(cli.yes);
    // axio's own directory holds the credential file. Without this, running
    // from a parent of it puts auth.json inside the workspace, where the read
    // tool would hand the key straight to the model. Sessions are named rather
    // than the whole state directory because the spill files beside them are
    // the model's own tool output, which it is meant to be able to read back.
    let policy = policy
        .protect(&axio_home())
        .protect(&state_dir().join("sessions"));
    notices.append(&mut policy_notices);

    // Resume before the agent exists: the header decides the model, because a
    // transcript's reasoning is only replayable under the model that minted it.
    let store = SessionStore::new(state_dir().join("sessions"));
    let (mut session, resumed, mut recorder) = match &cli.resume {
        Some(needle) => open_resumed(&store, needle, cli.model.as_deref(), &mut notices)?,
        None => {
            let session = Session::new(cwd.clone(), &cfg.model);
            let recorder = new_recorder(cli, &store, &session, label.unwrap_or(""), &mut notices);
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

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    let mut env_vars = axio_tools::proc::child_env();
    if sandbox_requested(cli, resolved) {
        // A tool that honours TMPDIR gets somewhere to write without the whole
        // shared temp directory being granted.
        let scratch = sandbox::scratch_dir(&state_dir());
        let _ = std::fs::create_dir_all(&scratch);
        for var in ["TMPDIR", "TMP", "TEMP"] {
            env_vars.retain(|(k, _)| k != var);
            env_vars.push((var.to_owned(), scratch.display().to_string()));
        }
    }

    let mut agent = Agent::new(
        provider,
        approver,
        session,
        cfg.clone(),
        vec![SystemBlock {
            text: system_prompt(&cwd),
        }],
        tx,
    )
    .with_policy(policy)
    .with_recorder(recorder)
    .with_env(ToolEnv { vars: env_vars });

    for tool in axio_tools::all() {
        agent.register_tool(tool);
    }

    // After the credential has been read and before any tool can run. That
    // ordering is what lets the sandbox exclude axio's own home entirely: a
    // shell command then has no path to `auth.json` even if every other guard
    // fails.
    Ok(Prepared {
        agent,
        events: rx,
        notices,
        resumed,
        model: cfg.model.clone(),
    })
}

/// The interactive surface.
///
/// `--yes` is honoured here too, and it is the only way an action goes
/// unasked-about: with a human present the approver is the human.
#[cfg(feature = "tui")]
async fn interactive(cli: &Cli, resolved: &Resolved) -> u8 {
    let (tui_approver, asks) = tui::approver();
    let approver: Arc<dyn axio_core::approver::Approver> = if cli.yes {
        Arc::new(NonInteractive::allow())
    } else {
        Arc::new(tui_approver)
    };

    let prepared = match prepare(cli, resolved, None, approver) {
        Ok(prepared) => prepared,
        Err(message) => {
            for notice in resolved.notices() {
                eprintln!("axio: {}", notice.message);
            }
            eprintln!("axio: {message}");
            return 1;
        }
    };

    match tui::run(
        prepared.agent,
        prepared.events,
        asks,
        prepared.resumed,
        prepared.notices,
        prepared.model,
    )
    .await
    {
        Ok(code) => code,
        Err(e) => {
            eprintln!("axio: the interface stopped: {e}");
            1
        }
    }
}

/// Built without the `tui` feature: the headless binary says so rather than
/// pretending the flag was wrong.
#[cfg(not(feature = "tui"))]
async fn interactive(_cli: &Cli, _resolved: &Resolved) -> u8 {
    eprintln!(
        "axio: this build has no interactive interface (built without the `tui` feature).\n\
         Run a single turn instead:  axio -p \"your prompt\"\n\
         or pipe input:              echo \"your prompt\" | axio"
    );
    2
}

async fn one_shot(cli: &Cli, resolved: &Resolved, prompt: String, stdout_is_tty: bool) -> u8 {
    let approver: Arc<dyn axio_core::approver::Approver> = Arc::new(if cli.yes {
        NonInteractive::allow()
    } else {
        NonInteractive::deny()
    });
    let Prepared {
        mut agent,
        events: mut rx,
        notices,
        resumed,
        ..
    } = match prepare(cli, resolved, Some(&prompt), approver) {
        Ok(prepared) => prepared,
        Err(message) => {
            // Config notices normally reach the user through `announce`, which
            // is downstream of here. On this path they are the explanation —
            // "no credential for `anthropic`" makes no sense to someone who
            // selected a different one until they are told their config was
            // rejected.
            for notice in resolved.notices() {
                eprintln!("axio: {}", notice.message);
            }
            eprintln!("axio: {message}");
            return 1;
        }
    };

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

    let mut refusals = Refusals::default();

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
                refusals.observe(&event);
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
                            refusals.observe(&event);
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
        // A turn can complete with its work refused: the model narrates a
        // success it was never allowed to perform, and prose is not something
        // a script can check. `&&` has to see that.
        0 => match outcome {
            TurnOutcome::Completed if refusals.count() > 0 => EXIT_REFUSED_ACTIONS,
            other => other.exit_code(),
        },
        code => code,
    }
}

/// The turn ran to completion, but policy refused at least one action.
const EXIT_REFUSED_ACTIONS: u8 = 5;

// ---------------------------------------------------------------- local modes

fn resolve_config(cli: &Cli) -> Resolved {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let user = Some(config_file_path());
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

fn explain(resolved: &Resolved, key: &str) -> u8 {
    print_notices(resolved);
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

/// A readable age from a unix timestamp in seconds.
///
/// Nobody reads unix seconds, and this listing is the only way to find the id
/// `--resume` wants — with several sessions from one project the label repeats
/// and the timestamp is the sole disambiguator.
fn age(started: &str) -> String {
    let Ok(then) = started.parse::<u64>() else {
        return started.to_owned();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(then);
    let secs = now.saturating_sub(then);
    match secs {
        0..=59 => "just now".to_owned(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
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
                // The directory too: with sessions from several projects in one
                // state directory, the label is otherwise the only disambiguator
                // and labels repeat.
                let project = h
                    .cwd
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                println!(
                    "{}  {:>9}  {:<14}  {:<16}  {}",
                    &h.id.to_string()[..8],
                    age(&h.started),
                    h.model,
                    project,
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

/// The user's configuration file. Always inside `axio_home`, so relocating
/// the home relocates everything axio owns rather than only half of it.
fn config_file_path() -> PathBuf {
    axio_home().join("config.toml")
}

/// axio's own directory: configuration, and the credential file.
fn axio_home() -> PathBuf {
    if let Some(explicit) = std::env::var_os("AXIO_HOME") {
        return PathBuf::from(explicit);
    }
    default_config_dir().unwrap_or_else(|| PathBuf::from(".axio"))
}

fn default_config_dir() -> Option<PathBuf> {
    use etcetera::BaseStrategy;
    etcetera::choose_base_strategy()
        .ok()
        .map(|s| s.config_dir().join("axio"))
}

/// Find a credential, environment first, then the store.
fn credential(provider: &str) -> Result<(Secret, auth::Source), String> {
    // Before anything about credentials: does this provider exist? Otherwise a
    // typo is diagnosed as a missing credential, the advice is to store one,
    // storing it succeeds, and only the next run says the name was never valid.
    if !auth::is_known(provider) {
        return Err(unknown_provider(provider));
    }

    let env: Vec<(String, String)> = std::env::vars().collect();
    let home = axio_home();
    if let Some(found) = auth::resolve(provider, &home, &env) {
        return Ok(found);
    }

    // Before explaining how to configure this provider, check whether another
    // one is already configured. "You have no credential" is unhelpful when
    // the real situation is "you have one, for something else" — which is what
    // happens to anyone whose only provider is not the default.
    let others: Vec<String> = auth::status(auth::PROVIDERS, &home, &env)
        .into_iter()
        .filter(|(name, source)| name != provider && source.is_some())
        .map(|(name, _)| name)
        .collect();

    let mut message = format!("no credential for `{provider}`.\n\n");

    if let Some(other) = others.first() {
        message.push_str(&format!(
            "`{other}` is configured, but `{provider}` is the one selected.\n\n\
             Use it for this command:\n    AXIO_PROVIDER={other} axio ...\n\n\
             Or make it the default:\n    [model]\n    provider = \"{other}\"\n\
             in {}\n\n",
            config_file_path().display()
        ));
    }

    message.push_str(&format!(
        "Store a credential for `{provider}`:\n    axio auth login --provider {provider}"
    ));
    // Only when there is a variable to name. Splicing the fallback prose into
    // an `export` line hands the user something that cannot be typed.
    if let Some(var) = auth::env_var_for(provider) {
        message.push_str(&format!(
            "\n\nOr set it for this shell:\n    export {var}=..."
        ));
    }
    Err(message)
}

fn unknown_provider(provider: &str) -> String {
    format!(
        "unknown provider `{provider}`; expected one of {}",
        auth::PROVIDERS
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn auth_command(action: &AuthAction) -> u8 {
    let home = axio_home();
    let env: Vec<(String, String)> = std::env::vars().collect();

    match action {
        AuthAction::Login { provider } => {
            // Refuse the name here rather than storing a credential that no run
            // can use and `auth status` cannot even list.
            if !auth::is_known(provider) {
                eprintln!("axio: {}", unknown_provider(provider));
                return 1;
            }
            // Read from stdin, never from an argument. A credential in argv is
            // visible in `ps` to every user on the machine and lands in shell
            // history besides.
            if std::io::stdin().is_terminal() {
                eprintln!(
                    "Paste the credential for `{provider}` and press enter.\n\
                     It will be visible as you type; pipe it in instead if that matters:\n\
                     \n    axio auth login --provider {provider} < key.txt\n"
                );
            }
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_err() {
                eprintln!("axio: could not read the credential");
                return 1;
            }
            let secret = Secret::new(input.trim());
            if secret.is_empty() {
                // Distinguish "you pressed enter" from "there was never a way
                // to type anything". The second happens whenever stdin is
                // /dev/null — a CI step, a task runner, an editor's terminal —
                // and the advice is completely different.
                if input.is_empty() && !std::io::stdin().is_terminal() {
                    eprintln!(
                        "axio: stdin is empty, so there was nothing to read.\n\n\
                         Pipe the credential in:\n    \
                         axio auth login --provider {provider} < key.txt\n\n\
                         Or run this from an interactive terminal to be prompted."
                    );
                } else {
                    eprintln!("axio: no credential given; nothing was stored");
                }
                return 1;
            }

            match auth::save(&home, provider, secret) {
                Ok(path) => {
                    println!(
                        "stored the credential for `{provider}` at {}",
                        path.display()
                    );
                    println!("{}", auth::protection_note());
                    if let Some(var) = auth::env_var_for(provider)
                        && env.iter().any(|(k, v)| k == var && !v.trim().is_empty())
                    {
                        // Otherwise the next run uses the variable and the user
                        // concludes the login did nothing.
                        println!(
                            "note: {var} is set in this shell and takes precedence over the stored credential"
                        );
                    }
                    0
                }
                Err(e) => {
                    eprintln!("axio: could not store the credential: {e}");
                    1
                }
            }
        }

        AuthAction::Status => {
            let rows = auth::status(auth::PROVIDERS, &home, &env);
            for (provider, source) in rows {
                match source {
                    Some(source) => println!("{provider:<18}  {}", source.describe()),
                    None => println!("{provider:<18}  not configured"),
                }
            }
            println!();
            println!("credential file: {}", auth::auth_path(&home).display());
            0
        }

        AuthAction::Logout { provider } => match auth::forget(&home, provider) {
            Ok(true) => {
                println!("removed the stored credential for `{provider}`");
                if let Some(var) = auth::env_var_for(provider)
                    && env.iter().any(|(k, v)| k == var && !v.trim().is_empty())
                {
                    println!("note: {var} is still set in this shell");
                }
                0
            }
            Ok(false) => {
                println!("no stored credential for `{provider}`");
                0
            }
            Err(e) => {
                eprintln!("axio: could not remove the credential: {e}");
                1
            }
        },
    }
}

/// Construct the provider the configuration names.
///
/// Two implementations selected by name. Adding a third would be the moment to
/// ask for a registry; two is not.
fn build_provider(resolved: &Resolved) -> Result<Arc<dyn axio_core::provider::Provider>, String> {
    let model = &resolved.config().model;
    // One lookup for every provider: the environment, then the store. A second
    // resolution path is how the two disagree about which key is in use.
    let (secret, _source) = credential(&model.provider)?;

    match model.provider.as_str() {
        "anthropic" => AnthropicProvider::new(secret.expose())
            .map(|p| match &model.base_url {
                // `model.base_url` was accepted, reported by `--explain`, and
                // ignored here — so a gateway or proxy endpoint silently went
                // to the public API instead.
                Some(url) => p.with_base_url(url.clone()),
                None => p,
            })
            .map(|p| Arc::new(p) as Arc<dyn axio_core::provider::Provider>)
            .map_err(|e| format!("could not start the http client: {e}")),
        "ollama" | "openai-compatible" => {
            let base = model
                .base_url
                .clone()
                .unwrap_or_else(|| OLLAMA_BASE.to_owned());
            OpenAiProvider::new(secret.expose(), base, model.provider.clone())
                .map(|p| Arc::new(p) as Arc<dyn axio_core::provider::Provider>)
                .map_err(|e| format!("could not start the http client: {e}"))
        }
        other => Err(unknown_provider(other)),
    }
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
         You are operating in a single turn; the user cannot answer questions mid-task.\n\n\
         Some actions require approval. If one is refused, that decision is final for this \
         run: do not retry it and do not invent an argument to bypass it. Never state that \
         work was done when the call that would have done it was refused or failed — say \
         plainly what you could not do and why.",
        cwd.display(),
        std::env::consts::OS,
    )
}

/// The prices the configured provider would actually charge against.
///
/// Read without constructing a provider, so `--doctor` still touches no
/// credential and opens no socket.
fn provider_prices(cfg: &axio_core::config::Config) -> Option<axio_core::provider::ModelInfo> {
    match cfg.model.provider.as_str() {
        "anthropic" => Some(axio_provider::anthropic::model_info(&cfg.model.name)),
        "ollama" | "openai-compatible" => Some(axio_provider::openai::model_info(&cfg.model.name)),
        _ => None,
    }
}

/// Everything the config loader complained about, on stderr.
///
/// The local modes return before any event stream exists, so a notice replayed
/// through `announce` never reaches them — which left `--doctor` and
/// `--explain`, whose whole job is explaining the configuration, as the two
/// surfaces that hid a rejected `[permissions] allow` or a discarded section.
fn print_notices(resolved: &Resolved) {
    for notice in resolved.notices() {
        eprintln!("axio: {}", notice.message);
    }
}

/// What axio can see, so a misconfiguration is one command away from obvious.
fn doctor(resolved: &Resolved) -> u8 {
    let mut out = std::io::stdout();
    let cfg = resolved.config();
    print_notices(resolved);
    let _ = writeln!(out, "axio {VERSION}");
    let _ = writeln!(out);

    let _ = writeln!(out, "credentials");
    // One resolution path, the same one the run itself uses, so doctor cannot
    // disagree with reality about which credential is in play.
    let env: Vec<(String, String)> = std::env::vars().collect();
    for (provider, source) in
        axio_core::auth::status(axio_core::auth::PROVIDERS, &axio_home(), &env)
    {
        match source {
            Some(source) => {
                let _ = writeln!(out, "  {provider:<18}  {}", source.describe());
            }
            None => {
                let _ = writeln!(out, "  {provider:<18}  not configured");
            }
        }
    }
    let _ = writeln!(
        out,
        "  -> axio auth login --provider {}",
        cfg.model.provider
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "model");
    let _ = writeln!(out, "  provider            {}", cfg.model.provider);
    let _ = writeln!(out, "  model               {}", cfg.model.name);
    let _ = writeln!(out, "  effort              {}", cfg.model.effort.as_wire());
    let _ = writeln!(out, "  max_tokens          {}", cfg.model.max_tokens);
    let _ = writeln!(out, "  max_steps           {}", cfg.budget.max_steps);
    // The endpoint a stale shell export points at is exactly the misconfiguration
    // this command exists to make obvious, and it was the one field not shown.
    let _ = writeln!(
        out,
        "  base_url            {}",
        cfg.model
            .base_url
            .as_deref()
            .unwrap_or("(provider default)")
    );
    match cfg.budget.max_usd_per_turn {
        Some(limit) => {
            let _ = writeln!(out, "  max_usd_per_turn    {limit:.2}");
        }
        None => {
            let _ = writeln!(out, "  max_usd_per_turn    (none)");
        }
    }
    let _ = writeln!(out);

    // From the provider that will actually be used, never a literal. A table
    // printed "because a stale price table is invisible until the bill" is worse
    // than nothing when it is a different provider's table — which is what a
    // hardcoded one becomes the moment a second provider exists.
    let prices = provider_prices(cfg);
    match prices {
        Some(info) if info.input_price > 0.0 || info.output_price > 0.0 => {
            let _ = writeln!(out, "prices (USD per million tokens)");
            let _ = writeln!(out, "  input               {:.2}", info.input_price);
            let _ = writeln!(out, "  output              {:.2}", info.output_price);
            let _ = writeln!(out, "  cache read          {:.2}", info.cache_read_price);
            let _ = writeln!(out, "  cache write         {:.2}", info.cache_write_price);
        }
        Some(_) => {
            let _ = writeln!(out, "prices");
            let _ = writeln!(
                out,
                "  this provider reports no prices, so recorded cost is always 0.00"
            );
            if cfg.budget.max_usd_per_turn.is_some() {
                let _ = writeln!(
                    out,
                    "  max_usd_per_turn is set but cannot trip — nothing measures spend here"
                );
            }
        }
        None => {
            let _ = writeln!(out, "prices");
            let _ = writeln!(out, "  unknown: the provider could not be constructed");
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "permissions");
    if cfg.permissions.allow.is_empty() && cfg.permissions.deny.is_empty() {
        let _ = writeln!(out, "  (no rules; the built-in deny list still applies)");
    }
    for rule in &cfg.permissions.deny {
        let _ = writeln!(out, "  deny                {rule}");
    }
    for rule in &cfg.permissions.allow {
        let _ = writeln!(out, "  allow               {rule}");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "paths");
    let _ = writeln!(
        out,
        "  user config         {}",
        config_file_path().display()
    );
    let _ = writeln!(out, "  axio home           {}", axio_home().display());
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
