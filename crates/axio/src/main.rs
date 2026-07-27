//! axio — a coding agent for the terminal.
//!
//! One binary, two surfaces. Which one runs is decided by how it was invoked,
//! not by a flag the caller has to remember.

mod cli;
mod credentials;
mod doctor;
mod paths;
mod render;
mod sandbox;
mod sessions;
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
use cli::{AuthAction, Cli, Command, VERSION};
use credentials::{auth_command, credential, unknown_provider};
use doctor::doctor;
use paths::{
    axio_home, config_file_path, explain, home_dir, print_notices, resolve_config, state_dir,
};
use render::{JsonlRenderer, PlainRenderer, Refusals, Renderer, Style};
use sessions::{list_sessions, new_recorder, now_ms, open_resumed};
use surface::Surface;
use tokio_util::sync::CancellationToken;

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

/// Reopen a session so the turn continues the same file.
type ResumedParts = (Session, bool, Recorder);

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
