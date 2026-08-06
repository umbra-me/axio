//! `axio session` — the supervisor, driven from the command line.
//!
//! Every capability a desktop surface will have is reachable here first. That
//! is not politeness to terminal users: a surface that can do something the CLI
//! cannot has stopped being a view of the same product, and the only way to
//! keep that honest is for both to drive `axio-supervisor` through the same
//! factory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axio_core::config::Resolved;
use axio_core::protocol::SessionId;
use axio_supervisor::{Disposition, Isolation, StartOptions, Supervisor, SupervisorConfig};

use crate::cli::SessionAction;
use crate::factory::LocalFactory;
use crate::paths::axio_home;

/// Where the supervisor keeps worktrees and its index.
///
/// Under `axio_home` rather than the state directory, and that is the whole
/// point. `state_dir` defaults to `temp/axio-<pid>` — a fresh directory per
/// process — which is right for spill files that only the running turn reads,
/// and fatal here: every `session start` would cut its worktree somewhere the
/// next `session list` could not find, and the branch holding the work would be
/// orphaned from the index describing it. What a supervisor owns outlives the
/// process that made it.
///
/// Outside every repository either way: a worktree inside the repo would appear
/// in its own `git status`, and one session could reach another's through a path
/// its `Workspace` never had to leave.
pub(crate) fn supervisor_root() -> PathBuf {
    axio_home().join("supervisor")
}

/// A supervisor for a surface that can live without one.
///
/// The interactive surface must open whether or not the index is readable —
/// refusing to start a terminal because a directory could not be listed trades
/// a whole feature for one that was already broken. `/new` reports the absence
/// when someone reaches for it.
/// Only the interactive surface asks for this — a headless build has no `/new`
/// to reach it from, and an ungated `pub(crate)` is dead code there.
#[cfg(feature = "tui")]
pub(crate) fn supervisor_for(resolved: &Resolved, yes: bool) -> Option<Arc<Supervisor>> {
    match build(resolved, yes) {
        Ok(supervisor) => Some(Arc::new(supervisor)),
        Err(_) => None,
    }
}

fn build(resolved: &Resolved, yes: bool) -> Result<Supervisor, String> {
    // Not `build_provider`, which fails closed on a missing credential. Cutting
    // a worktree is a filesystem question and asking it should not require a
    // key, so an unusable provider becomes one that explains itself when a turn
    // actually needs it — exactly what the interactive surface does. The failure
    // still happens, at the point where it is about the model.
    let (provider, _why) = crate::provider::build_or_explain(resolved);
    let factory = Arc::new(LocalFactory::new(resolved.clone(), provider).unattended(yes));
    let (supervisor, _events) = Supervisor::new(
        SupervisorConfig {
            state_root: supervisor_root(),
            worktree: resolved.config().worktree.clone(),
        },
        factory,
    )
    .map_err(|e| e.to_string())?;
    Ok(supervisor)
}

/// A supervisor that reads the index but never starts anything.
///
/// `list`, `diff` and `close` do not need a provider, and demanding one would
/// make "what did that session change" fail on a machine with no credential —
/// a question about a directory answered by refusing to look at it.
fn read_only() -> Result<Supervisor, String> {
    struct NoFactory;
    #[async_trait::async_trait]
    impl axio_supervisor::AgentFactory for NoFactory {
        async fn build(
            &self,
            _request: axio_supervisor::AgentRequest,
        ) -> Result<axio_core::Agent, String> {
            Err("this command does not start sessions".to_owned())
        }
    }
    let (supervisor, _events) = Supervisor::new(
        SupervisorConfig {
            state_root: supervisor_root(),
            worktree: axio_core::config::WorktreeSection::default(),
        },
        Arc::new(NoFactory),
    )
    .map_err(|e| e.to_string())?;
    Ok(supervisor)
}

pub(crate) async fn session_command(action: &SessionAction, resolved: &Resolved, yes: bool) -> u8 {
    let outcome = match action {
        SessionAction::Start {
            prompt,
            repo,
            direct,
            discard,
            ..
        } => {
            start(
                resolved,
                yes,
                prompt.as_deref(),
                repo.as_deref(),
                *direct,
                *discard,
            )
            .await
        }
        SessionAction::List { repo, json } => list(repo.as_deref(), *json),
        SessionAction::Diff { id } => diff(id).await,
        SessionAction::Close { id, discard } => close(id, *discard).await,
    };
    match outcome {
        Ok(code) => code,
        Err(message) => {
            eprintln!("axio: {message}");
            1
        }
    }
}

async fn start(
    resolved: &Resolved,
    yes: bool,
    prompt: Option<&str>,
    repo: Option<&Path>,
    direct: bool,
    discard: bool,
) -> Result<u8, String> {
    let supervisor = build(resolved, yes)?;
    let where_ = repo
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);

    let handle = supervisor
        .start(
            &where_,
            StartOptions {
                isolation: direct.then_some(Isolation::Direct),
                label: prompt.map(str::to_owned),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    println!("session {}", handle.session);
    match &handle.checkout.branch {
        Some(branch) => println!("  branch    {branch}"),
        None => println!("  branch    (none — running in the repository itself)"),
    }
    println!("  workspace {}", handle.checkout.path.display());

    let Some(prompt) = prompt else {
        // No prompt is a deliberate mode: cut the worktree, leave it idle, and
        // let something else drive it. Saying so beats a silent no-op.
        println!("\nno prompt given, so nothing was run. The worktree is ready.");
        return Ok(0);
    };

    // Unattended by construction: nobody is watching this session, so anything
    // policy cannot decide alone is denied unless `--yes` was given. The pooled
    // queue exists for surfaces that can ask; a one-shot command cannot.
    let outcome = handle.turn(prompt).await.map_err(|e| e.to_string())?;
    println!("\n{outcome:?}");

    let changed = handle.checkout.status().await.map_err(|e| e.to_string())?;
    println!("{} file(s) changed", changed.len());
    for line in changed.iter().take(20) {
        println!("  {line}");
    }
    if changed.len() > 20 {
        println!("  … and {} more", changed.len() - 20);
    }

    if discard {
        supervisor
            .close(handle.session, Disposition::Discard)
            .await
            .map_err(|e| e.to_string())?;
        println!("\nworktree removed");
    } else {
        supervisor
            .close(handle.session, Disposition::Keep)
            .await
            .map_err(|e| e.to_string())?;
        println!(
            "\nkept. `axio session diff {}` to read it.",
            short(handle.session)
        );
    }
    Ok(outcome.exit_code())
}

fn list(repo: Option<&Path>, json: bool) -> Result<u8, String> {
    let supervisor = read_only()?;
    let entries = match repo {
        Some(path) => {
            let root = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            let id = axio_supervisor::ProjectId::of(&root);
            supervisor.history_for(&id)
        }
        None => supervisor.history(),
    };

    if json {
        for entry in &entries {
            match serde_json::to_string(entry) {
                Ok(line) => println!("{line}"),
                Err(e) => eprintln!("axio: could not serialise {}: {e}", entry.session),
            }
        }
        return Ok(0);
    }

    if entries.is_empty() {
        println!("no supervised sessions yet");
        return Ok(0);
    }

    let mut project = String::new();
    for entry in &entries {
        if entry.project_name != project {
            project = entry.project_name.clone();
            println!("\n{}  {}", project, entry.project_root.display());
        }
        println!(
            "  {}  {:<7}  {}",
            short(entry.session),
            if entry.is_open() { "open" } else { "closed" },
            entry.label.as_deref().unwrap_or("(no prompt)")
        );
    }
    Ok(0)
}

async fn diff(needle: &str) -> Result<u8, String> {
    let supervisor = read_only()?;
    let entry = find(&supervisor, needle)?;
    let text = entry.checkout().diff().await.map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        // A real outcome, and a different one from a failure to read the
        // worktree. An empty pane would read as the latter.
        println!("that session changed nothing");
    } else {
        println!("{text}");
    }
    Ok(0)
}

async fn close(needle: &str, discard: bool) -> Result<u8, String> {
    let supervisor = read_only()?;
    let entry = find(&supervisor, needle)?;
    let disposition = if discard {
        Disposition::Discard
    } else {
        Disposition::Keep
    };
    entry
        .checkout()
        .close(disposition)
        .await
        .map_err(|e| e.to_string())?;
    println!(
        "closed {}{}",
        short(entry.session),
        if discard {
            " and deleted its branch"
        } else {
            ""
        }
    );
    Ok(0)
}

/// Resolve a full id or an unambiguous prefix.
///
/// The same affordance `--resume` has, for the same reason: a 26-character
/// identifier typed by hand is a usability failure.
fn find(supervisor: &Supervisor, needle: &str) -> Result<axio_supervisor::IndexEntry, String> {
    let typed = needle.trim();
    let upper = typed.to_ascii_uppercase();
    let matches: Vec<axio_supervisor::IndexEntry> = supervisor
        .history()
        .into_iter()
        .filter(|e| e.session.to_string().starts_with(&upper))
        .collect();
    match matches.len() {
        0 => Err(format!("no session matches `{typed}`")),
        1 => Ok(matches.into_iter().next().expect("just counted one")),
        n => Err(format!(
            "`{typed}` matches {n} sessions; give more characters"
        )),
    }
}

fn short(id: SessionId) -> String {
    id.to_string().chars().take(8).collect()
}
