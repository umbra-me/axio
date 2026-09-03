//! Landing a session's work, the way this window does it.
//!
//! The supervisor stops at the branch on purpose: merging, pushing and opening
//! a pull request are workflows, and a supervisor that picked one would be
//! wrong for the other two. This surface picks all three and says which is
//! which. Every step is git or `gh` run in the worktree or the repository,
//! through the supervisor's own runner — credentials stripped, no prompt — so
//! nothing here can hang a window on a password nobody is there to type.
//!
//! Uncommitted work is committed first, under the session's name. An agent
//! rarely commits, and a "merge" that silently landed nothing because the
//! change was still in the working tree would be the worst kind of success.

use std::path::Path;

use axio_supervisor::git;

use crate::model::{AppError, LandAction, LandOutcome, LandingView};

use super::AppState;

impl AppState {
    /// Where a session's branch stands against the repository it came from.
    pub async fn landing(&self, session_id: &str) -> Result<LandingView, AppError> {
        let entry = self.entry(session_id)?;
        let checkout = entry.checkout();
        let base = git(&entry.project_root, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .map_err(|e| AppError::Supervisor(e.to_string()))?;
        let changed = checkout
            .status()
            .await
            .map_err(|e| AppError::Supervisor(e.to_string()))?;
        let ahead = match &checkout.branch {
            Some(branch) => git(
                &entry.project_root,
                &["rev-list", "--count", &format!("{base}..{branch}")],
            )
            .await
            .ok()
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0),
            None => 0,
        };
        let remote = git(&entry.project_root, &["remote", "get-url", "origin"])
            .await
            .ok()
            .filter(|r| !r.trim().is_empty());
        Ok(LandingView {
            branch: checkout.branch.clone(),
            base,
            changed,
            ahead,
            remote,
            can_pr: on_path("gh"),
        })
    }

    /// Land a session's branch. See `LandAction` for what each does.
    pub async fn land(
        &self,
        session_id: &str,
        action: LandAction,
    ) -> Result<LandOutcome, AppError> {
        let entry = self.entry(session_id)?;
        let checkout = entry.checkout();
        let Some(branch) = checkout.branch.clone() else {
            return Err(AppError::NoRepository(
                "this session worked in the checkout itself; there is no branch to land".to_owned(),
            ));
        };
        let name = entry
            .title
            .clone()
            .or_else(|| entry.label.clone())
            .unwrap_or_else(|| format!("axio session {}", entry.session));
        let committed = commit_if_dirty(&checkout.path, &name).await?;

        let sup = |e: axio_supervisor::SupervisorError| AppError::Supervisor(e.to_string());
        match action {
            LandAction::Merge => {
                let base = git(&entry.project_root, &["rev-parse", "--abbrev-ref", "HEAD"])
                    .await
                    .map_err(sup)?;
                git(
                    &entry.project_root,
                    &["merge", "--no-ff", "--no-edit", &branch],
                )
                .await
                .map_err(sup)?;
                Ok(LandOutcome {
                    message: format!(
                        "merged {branch} into {base}{}",
                        if committed {
                            " (uncommitted work was committed first)"
                        } else {
                            ""
                        }
                    ),
                    url: None,
                })
            }
            LandAction::Push => {
                git(&checkout.path, &["push", "-u", "origin", &branch])
                    .await
                    .map_err(sup)?;
                Ok(LandOutcome {
                    message: format!("pushed {branch} to origin"),
                    url: None,
                })
            }
            LandAction::PullRequest => {
                git(&checkout.path, &["push", "-u", "origin", &branch])
                    .await
                    .map_err(sup)?;
                let out = tokio::process::Command::new("gh")
                    .args(["pr", "create", "--fill", "--head", &branch])
                    .current_dir(&checkout.path)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .map_err(|e| AppError::Supervisor(format!("could not run gh: {e}")))?;
                if !out.status.success() {
                    return Err(AppError::Supervisor(format!(
                        "gh pr create failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                let url = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .rev()
                    .find(|l| l.starts_with("http"))
                    .map(str::to_owned);
                Ok(LandOutcome {
                    message: format!("opened a pull request for {branch}"),
                    url,
                })
            }
        }
    }

    fn entry(&self, session_id: &str) -> Result<axio_supervisor::IndexEntry, AppError> {
        let supervisor = self.supervisor()?;
        let id: axio_core::protocol::SessionId = session_id
            .parse()
            .map_err(|_| AppError::NoSuchSession(format!("`{session_id}` is not a session id")))?;
        supervisor
            .history()
            .into_iter()
            .find(|e| e.session == id)
            .ok_or_else(|| AppError::NoSuchSession(format!("no session {session_id}")))
    }
}

/// Commit everything in the worktree under `message`, if there is anything.
/// `Ok(true)` when a commit was made.
async fn commit_if_dirty(worktree: &Path, message: &str) -> Result<bool, AppError> {
    let sup = |e: axio_supervisor::SupervisorError| AppError::Supervisor(e.to_string());
    let status = git(worktree, &["status", "--porcelain"])
        .await
        .map_err(sup)?;
    if status.trim().is_empty() {
        return Ok(false);
    }
    git(worktree, &["add", "-A"]).await.map_err(sup)?;
    git(worktree, &["commit", "-q", "-m", message])
        .await
        .map_err(sup)?;
    Ok(true)
}

/// Whether an executable of this name is on `PATH`.
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let bare = dir.join(name);
        bare.is_file() || dir.join(format!("{name}.exe")).is_file()
    })
}

/// Show a path in the platform's file manager, or open it in the editor the
/// settings name. Host-side, never on the model's behalf.
pub fn reveal(path: &str, editor: Option<&str>) -> Result<(), AppError> {
    let spawn = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|e| AppError::Supervisor(format!("could not run {program}: {e}")))
    };
    match editor {
        Some(editor) if !editor.trim().is_empty() => {
            let mut parts = editor.split_whitespace();
            let program = parts.next().unwrap_or("code");
            let mut args: Vec<&str> = parts.collect();
            args.push(path);
            spawn(program, &args)
        }
        _ => {
            #[cfg(target_os = "macos")]
            return spawn("open", &[path]);
            #[cfg(target_os = "windows")]
            return spawn("explorer", &[path]);
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            return spawn("xdg-open", &[path]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn landing_without_a_supervisor_is_an_error_not_a_panic() {
        let state = AppState::unavailable("nope");
        assert!(state.landing("01K").await.is_err());
        assert!(state.land("01K", LandAction::Merge).await.is_err());
    }

    #[test]
    fn on_path_finds_git_and_not_a_made_up_name() {
        assert!(
            on_path("git"),
            "git is on every machine that runs this suite"
        );
        assert!(!on_path("axio-no-such-program-zz"));
    }
}
