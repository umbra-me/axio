//! Running git.
//!
//! Shelled out to rather than linked. libgit2 compiles C source, and a default
//! `cargo install axio` is promised not to need a C toolchain — the same
//! constraint `deny.toml` already enforces against aws-lc-sys. git is on the
//! machine of anyone who has a repository to supervise.
//!
//! This is host-side work: it happens between turns, on paths the supervisor
//! chose, and no tool can reach it. `axio-tools` remains the only crate that
//! spawns a process **on the model's behalf**, which is the property that
//! matters — a git invocation here is not something the model can ask for.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use crate::error::{Result, SupervisorError};

/// The environment git runs in.
///
/// Credentials are stripped for one specific reason: git runs hooks, and hooks
/// are code the repository author wrote. `post-checkout` firing with
/// `ANTHROPIC_API_KEY` in its environment is a credential handed to a clone.
/// The provider list is asked rather than hard-coded, so adding a provider
/// cannot leave its key behind — the same derivation `axio-tools` uses.
fn env() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> =
        std::env::vars().filter(|(k, _)| !is_stripped(k)).collect();
    // Never block waiting for a credential nobody is there to type. Without
    // this a `worktree add` against a repository with a remote callback can
    // hang forever, and in a window there is nothing to Ctrl-C.
    out.push(("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()));
    out.push(("NO_COLOR".to_owned(), "1".to_owned()));
    out
}

fn is_stripped(key: &str) -> bool {
    if key.starts_with("AXIO_") || key == "ANTHROPIC_AUTH_TOKEN" {
        return true;
    }
    axio_core::auth::PROVIDERS
        .iter()
        .filter_map(|p| axio_core::auth::env_var_for(p))
        .any(|var| var == key)
}

/// Run git in `cwd` and return its trimmed stdout.
///
/// A non-zero exit is an error carrying git's own stderr. Guessing at what a
/// git failure meant is how a wrong diagnosis reaches the user; git already
/// wrote a better one.
pub(crate) async fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env() {
        cmd.env(k, v);
    }

    let output = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SupervisorError::GitMissing(e.to_string())
        } else {
            SupervisorError::io(format!("git {}", args.join(" ")), e)
        }
    })?;

    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(SupervisorError::Git {
            args: args.join(" "),
            message: if message.is_empty() {
                format!("exited with {}", output.status)
            } else {
                message
            },
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Whether git answers at all, and whether `cwd` is in a repository.
pub(crate) async fn toplevel(cwd: &Path) -> Result<std::path::PathBuf> {
    match run(cwd, &["rev-parse", "--show-toplevel"]).await {
        Ok(path) => Ok(std::path::PathBuf::from(path)),
        Err(SupervisorError::Git { .. }) => Err(SupervisorError::NotARepository(cwd.to_path_buf())),
        Err(other) => Err(other),
    }
}

/// A repository with no commits cannot be branched from.
///
/// Checked before `worktree add` rather than after, so the error names the
/// actual problem instead of relaying git's "invalid reference: HEAD".
pub(crate) async fn has_commits(repo: &Path) -> bool {
    run(repo, &["rev-parse", "--verify", "HEAD"]).await.is_ok()
}

/// Whether a branch name is already taken.
pub(crate) async fn branch_exists(repo: &Path, branch: &str) -> bool {
    run(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_providers_credential_is_kept_from_a_hook() {
        // Hooks are code from the repository. The list is derived rather than
        // written down, so a new provider is covered without anyone
        // remembering this file exists.
        for provider in axio_core::auth::PROVIDERS {
            if let Some(var) = axio_core::auth::env_var_for(provider) {
                assert!(is_stripped(var), "{var} would reach a git hook");
            }
        }
        assert!(is_stripped("AXIO_HOME"));
        assert!(!is_stripped("PATH"), "git still needs a PATH");
    }

    #[tokio::test]
    async fn a_directory_outside_a_repository_is_named_as_such() {
        let dir = tempfile::tempdir().unwrap();
        match toplevel(dir.path()).await {
            Err(SupervisorError::NotARepository(_)) => {}
            other => panic!("expected NotARepository, got {other:?}"),
        }
    }
}
