//! axio — a coding agent for the terminal.
//!
//! One binary, two surfaces. Which one runs is decided by how it was invoked,
//! not by a flag the caller has to remember.

mod render;
mod surface;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use axio_core::agent::{Agent, RuntimeConfig};
use axio_core::approver::NonInteractive;
use axio_core::protocol::TurnOutcome;
use axio_core::provider::SystemBlock;
use axio_core::session::Session;
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
    if cli.doctor {
        return doctor();
    }

    let stdin_is_tty = std::io::stdin().is_terminal();
    let stdout_is_tty = std::io::stdout().is_terminal();

    // Read piped stdin before deciding, so `-p` can append it.
    let piped = read_stdin(stdin_is_tty, cli.prompt.is_some());

    match Surface::select(cli.prompt.as_deref(), piped.as_deref(), stdin_is_tty) {
        Surface::OneShot(prompt) => one_shot(&cli, prompt, stdout_is_tty).await,
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

async fn one_shot(cli: &Cli, prompt: String, stdout_is_tty: bool) -> u8 {
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
    let cfg = RuntimeConfig::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let mut agent = Agent::new(
        provider,
        Arc::new(if cli.yes {
            NonInteractive::allow()
        } else {
            NonInteractive::deny()
        }),
        Session::new(cwd.clone(), &cfg.model),
        cfg.clone(),
        vec![SystemBlock {
            text: system_prompt(&cwd),
        }],
        tx,
    );

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

    agent.announce(false);

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

fn system_prompt(cwd: &std::path::Path) -> String {
    format!(
        "You are axio, a coding agent running in a terminal.\n\
         Working directory: {}\n\
         Platform: {}\n\n\
         Keep responses focused, brief, and concise. Lead with the outcome, then the detail.\n\
         Deliver what was asked at the scope intended: make routine judgement calls yourself, \
         and check in only when different readings would lead to materially different work.\n\
         You are operating in a single turn; the user cannot answer questions mid-task.",
        cwd.display(),
        std::env::consts::OS,
    )
}

/// What axio can see, so a misconfiguration is one command away from obvious.
fn doctor() -> u8 {
    let mut out = std::io::stdout();
    let cfg = RuntimeConfig::default();
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
    let _ = writeln!(out, "  model               {}", cfg.model);
    let _ = writeln!(out, "  effort              {}", cfg.effort.as_wire());
    let _ = writeln!(out, "  max_tokens          {}", cfg.max_tokens);
    let _ = writeln!(out, "  max_steps           {}", cfg.max_steps);
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
