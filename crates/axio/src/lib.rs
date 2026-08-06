//! axio — a coding agent for the terminal.
//!
//! One binary, two surfaces. Which one runs is decided by how it was invoked,
//! not by a flag the caller has to remember.

pub mod factory;

/// Resolve configuration the way a surface with no flags would.
///
/// Exposed so a second surface reads the same layers in the same order — a
/// desktop application that resolved its own would be one `AXIO_*` variable
/// away from disagreeing with the terminal about which model is in use.
pub fn resolve_for_surface() -> Resolved {
    resolve_config(&Cli::parse_from(["axio"]))
}

/// A provider, or one that explains what is missing when a turn needs it.
pub fn provider_or_explain(
    resolved: &Resolved,
) -> (Arc<dyn axio_core::provider::Provider>, Option<String>) {
    provider::build_or_explain(resolved)
}

/// Where supervised worktrees and the session index live.
pub fn supervisor_root() -> PathBuf {
    session_cmd::supervisor_root()
}

mod input;
mod surfaces;

mod cli;
mod credentials;
// Only the interactive surface can switch provider mid-session, so only it
// has a default to save.
mod cost;
#[cfg(feature = "tui")]
mod defaults;
mod doctor;
mod paths;
mod probe;
mod provider;
mod quota;
mod render;
mod sandbox;
mod session_cmd;
mod sessions;
mod surface;
#[cfg(feature = "tui")]
mod tui;
mod workspace;

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
use input::read_stdin;
use paths::{
    axio_home, config_file_path, explain, home_dir, print_notices, resolve_config, state_dir,
};
use probe::probe;
use render::{JsonlRenderer, PlainRenderer, Refusals, Renderer, Style};
use sessions::{list_sessions, new_recorder, now_ms, open_resumed};
use surface::Surface;
use surfaces::{ResumedParts, interactive, one_shot};
use tokio_util::sync::CancellationToken;

/// The binary's whole body, so `main.rs` is three lines and everything else is
/// reachable from a library.
///
/// The split exists for one reason: a second surface. `prepare` resolves the
/// configuration, builds the provider, registers the tools and protects the
/// directories, and a desktop application that reimplemented any of that would
/// be a second product wearing the same name. A binary-only crate cannot be
/// reused, so this one is not binary-only.
pub fn main_entry() -> std::process::ExitCode {
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
/// touch nothing a command could, so neither is worth confining — and
/// confining the first would break it.
fn confine(cli: &Cli) -> Option<Notice> {
    let resolved = resolve_config(cli);
    let runs_a_turn =
        cli.command.is_none() && !cli.doctor && !cli.probe && cli.explain.is_none() && !cli.list;
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
    if let Some(Command::Cost {
        by,
        json,
        diagnose,
        calendar,
        cached,
        wide,
        limit,
        import_prices,
    }) = &cli.command
    {
        return match import_prices {
            Some(path) => cost::import_prices(path),
            None => cost::cost_command(*by, *json, *diagnose, *calendar, *cached, *wide, *limit),
        };
    }
    if let Some(Command::Session { action }) = &cli.command {
        return session_cmd::session_command(action, &resolved, cli.yes).await;
    }
    if let Some(Command::Quota {
        provider,
        json,
        diagnose,
    }) = &cli.command
    {
        return quota::quota_command(provider.as_deref(), *json, *diagnose).await;
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

    // Deliberately below that block and not inside it. `--probe` resolves a
    // credential and opens a socket, which is exactly what the modes above
    // promise not to do — and is why it is its own flag rather than another
    // section of `--doctor`.
    if cli.probe {
        return probe(&resolved).await;
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

// ---------------------------------------------------------------- local modes
