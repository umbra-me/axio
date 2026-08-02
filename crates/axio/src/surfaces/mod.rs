//! The two ways a turn is driven, and what they share.
//!
//! Which one runs is decided by how axio was invoked, never by a flag the
//! caller has to remember. Both build the same agent from the same resolved
//! configuration; they differ only in what consumes its events.

mod prompt;

use super::*;
use prompt::system_prompt;

/// Everything a surface needs, built once so the two of them cannot drift.
///
/// The interactive path and the one-shot path resolve the same configuration,
/// protect the same directories and register the same tools. Two copies of this
/// is how one surface quietly ends up with a permission rule the other does not
/// apply.
pub(crate) struct Prepared {
    agent: Agent,
    events: tokio::sync::mpsc::UnboundedReceiver<axio_core::protocol::Event>,
    notices: Vec<Notice>,
    resumed: bool,
    /// Shown in the interactive banner. The headless build has no banner, and
    /// CI compiles it with `-D warnings`.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    model: String,
    /// What `/status` shows besides the model. Built here rather than in the
    /// surface so it is read off the same resolved configuration the agent was
    /// built from, and cannot describe a session that is not the one running.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    facts: Vec<String>,
}

pub(crate) fn prepare(
    cli: &Cli,
    resolved: &Resolved,
    label: Option<&str>,
    approver: Arc<dyn axio_core::approver::Approver>,
) -> Result<Prepared, String> {
    let provider: Arc<dyn axio_core::provider::Provider> =
        crate::provider::build_provider(resolved)?;
    prepare_with_provider(cli, resolved, label, approver, provider)
}

fn prepare_with_provider(
    cli: &Cli,
    resolved: &Resolved,
    label: Option<&str>,
    approver: Arc<dyn axio_core::approver::Approver>,
    provider: Arc<dyn axio_core::provider::Provider>,
) -> Result<Prepared, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut cfg = resolved.runtime();
    cfg.spill_dir = Some(state_dir().join("outputs"));

    let mut notices: Vec<Notice> = resolved.notices().to_vec();
    // Before anything else notices-worthy, because it explains the shape of
    // every search the session is about to run.
    if let Some(crowded) = workspace::crowded_notice(&cwd) {
        notices.push(Notice::info(crowded));
    }
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
        facts: doctor::session_facts(resolved, &cwd),
    })
}

/// Every provider, and whether it has a credential.
///
/// Listed even when it does not: a provider missing from the list reads as one
/// axio cannot reach, when the answer is a sign-in away.
#[cfg(feature = "tui")]
pub(crate) fn offers() -> Vec<tui::Offer> {
    let env: Vec<(String, String)> = std::env::vars().collect();
    let home = axio_home();
    axio_core::auth::PROVIDERS
        .iter()
        .map(|name| tui::Offer {
            name: (*name).to_owned(),
            ready: axio_core::auth::resolve(name, &home, &env).is_some(),
        })
        .collect()
}

/// The interactive surface.
///
/// `--yes` is honoured here too, and it is the only way an action goes
/// unasked-about: with a human present the approver is the human.
#[cfg(feature = "tui")]
pub(crate) async fn interactive(cli: &Cli, resolved: &Resolved) -> u8 {
    let (tui_approver, asks) = tui::approver();
    let approver: Arc<dyn axio_core::approver::Approver> = if cli.yes {
        Arc::new(NonInteractive::allow())
    } else {
        Arc::new(tui_approver)
    };

    let (provider, unavailable) = crate::provider::build_for_tui(resolved);
    let mut prepared = match prepare_with_provider(cli, resolved, None, approver, provider) {
        Ok(prepared) => prepared,
        Err(message) => {
            for notice in resolved.notices() {
                eprintln!("axio: {}", notice.message);
            }
            eprintln!("axio: {message}");
            return 1;
        }
    };
    if let Some(why) = unavailable {
        prepared.notices.push(Notice::warn(format!(
            "the configured provider is unavailable ({why}); use /login, then bare /model"
        )));
    }

    match tui::run(
        prepared.agent,
        prepared.events,
        asks,
        tui::Setup {
            resumed: prepared.resumed,
            notices: prepared.notices,
            model: prepared.model,
            facts: prepared.facts,
            factory: crate::provider::factory(resolved),
            offers: offers(),
        },
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
pub(crate) async fn interactive(_cli: &Cli, _resolved: &Resolved) -> u8 {
    eprintln!(
        "axio: this build has no interactive interface (built without the `tui` feature).\n\
         Run a single turn instead:  axio -p \"your prompt\"\n\
         or pipe input:              echo \"your prompt\" | axio"
    );
    2
}

pub(crate) async fn one_shot(
    cli: &Cli,
    resolved: &Resolved,
    prompt: String,
    stdout_is_tty: bool,
) -> u8 {
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
pub(crate) const EXIT_REFUSED_ACTIONS: u8 = 5;

/// Reopen a session so the turn continues the same file.
pub(crate) type ResumedParts = (Session, bool, Recorder);
