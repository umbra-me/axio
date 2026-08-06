//! Where a session's work actually lands.
//!
//! Isolation is the default and the reason the crate exists: five agents
//! editing one checkout is not five sessions, it is one corrupted tree. Each
//! session gets a git worktree on its own branch, so what an agent did is a
//! branch you can read, land or delete — and the checkout you were using never
//! moved.
//!
//! Merging is deliberately **not** here. Landing work is a workflow — some
//! people merge, some open a pull request, some cherry-pick — and a supervisor
//! that picked one would be wrong for the other two. What is here is the branch
//! name, the status and the diff: everything a caller needs to land it whichever
//! way it lands things.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::error::{Result, SupervisorError};
use crate::git;
use crate::project::Project;

/// Whether a session works in its own checkout or in yours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// A git worktree on its own branch. The default.
    Worktree,
    /// The repository as it sits. Chosen explicitly, never fallen back to:
    /// isolation that silently did not happen is the one failure nobody
    /// notices until two agents have overwritten each other.
    Direct,
}

/// What to do with a worktree when its session closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Leave the worktree and its branch. The default, because the work is the
    /// point and a session ending is not a decision to throw it away.
    Keep,
    /// Remove the worktree and delete the branch. Refuses if the branch holds
    /// commits that are not on the base, so "discard" cannot silently mean
    /// "lose an afternoon".
    Discard,
}

/// The directory a session's tools are confined to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkout {
    /// Where the agent works. This becomes the `Workspace` root, so every tool
    /// is confined to it by machinery that already exists.
    pub path: PathBuf,
    /// The repository this came from.
    pub repo: PathBuf,
    /// `None` when [`Isolation::Direct`].
    pub branch: Option<String>,
    pub isolation: Isolation,
}

impl Checkout {
    /// Use the repository as it is.
    pub fn direct(project: &Project) -> Self {
        Self {
            path: project.root.clone(),
            repo: project.root.clone(),
            branch: None,
            isolation: Isolation::Direct,
        }
    }

    /// Cut a fresh worktree on a fresh branch.
    ///
    /// `id` names both the directory and the branch, so the two can never drift
    /// apart and neither needs to be looked up from the other. It is the full
    /// ULID rather than a readable prefix because a ULID's first ten characters
    /// are its millisecond timestamp — two sessions started in the same
    /// millisecond would collide, which is exactly the case a queue of agents
    /// produces.
    pub async fn worktree(
        project: &Project,
        root: &Path,
        id: Ulid,
        branch_prefix: &str,
    ) -> Result<Self> {
        if !git::has_commits(&project.root).await {
            return Err(SupervisorError::NoCommits(project.root.clone()));
        }

        let branch = format!("{branch_prefix}{}", id.to_string().to_lowercase());
        if git::branch_exists(&project.root, &branch).await {
            return Err(SupervisorError::Git {
                args: format!("worktree add -b {branch}"),
                message: format!("branch `{branch}` already exists"),
            });
        }

        let dir = root.join(project.id.as_str()).join(id.to_string());
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SupervisorError::io(format!("creating {}", parent.display()), e))?;
        }

        // `HEAD` rather than a named base: whatever the repository is currently
        // on is what someone starting a session there means by "from here", and
        // guessing at `main` is wrong in every repository that calls it
        // something else.
        git::run(
            &project.root,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                &dir.to_string_lossy(),
                "HEAD",
            ],
        )
        .await?;

        // Recorded exactly as git was given it, deliberately not canonicalised.
        // git registers a worktree under the path it was handed, and on Windows
        // `canonicalize` returns the `\\?\` extended-length form — which is a
        // different string, so `worktree remove` would fail to match its own
        // registration and every closed session would leave one behind.
        // `Workspace` canonicalises for confinement on its own, so nothing
        // downstream needs this to be the real path.
        Ok(Self {
            path: dir,
            repo: project.root.clone(),
            branch: Some(branch),
            isolation: Isolation::Worktree,
        })
    }

    /// Porcelain status, one entry per changed path.
    ///
    /// Empty means the agent changed nothing — which is a real outcome worth
    /// showing rather than an empty diff view that reads like a failure.
    pub async fn status(&self) -> Result<Vec<String>> {
        let out = git::run(&self.path, &["status", "--porcelain"]).await?;
        Ok(out.lines().map(|l| l.trim_end().to_owned()).collect())
    }

    /// Everything this session did, as a unified diff against where it started.
    ///
    /// Includes untracked files by way of `--no-index`-free staging: they are
    /// added to the index first with `add -N`, which records the path without
    /// its content, so a file the agent created appears in the diff instead of
    /// silently not existing. This is the same blind spot `scripts/limits.sh`
    /// documents for untracked files, and it would be worse here — a review
    /// surface that omits new files shows an incomplete change as a complete
    /// one.
    pub async fn diff(&self) -> Result<String> {
        if self.isolation == Isolation::Worktree {
            let _ = git::run(&self.path, &["add", "-N", "."]).await;
        }
        git::run(&self.path, &["diff", "HEAD"]).await
    }

    /// Commits on this branch that are not on the commit it started from.
    async fn unmerged_commits(&self) -> Result<u32> {
        let Some(branch) = &self.branch else {
            return Ok(0);
        };
        // A session branch has no upstream unless someone pushed it, so the
        // comparison that works in every case is against where the repository
        // itself is. Naming a base branch would guess, and guessing `main` is
        // wrong in every repository that calls it something else.
        let base = match git::run(
            &self.path,
            &["merge-base", "HEAD", &format!("{branch}@{{u}}")],
        )
        .await
        {
            Ok(base) => base,
            Err(_) => git::run(&self.repo, &["rev-parse", "HEAD"]).await?,
        };
        let count = git::run(
            &self.path,
            &["rev-list", "--count", &format!("{base}..HEAD")],
        )
        .await?;
        Ok(count.parse().unwrap_or(0))
    }

    /// On [`Disposition::Discard`], remove the worktree and its branch. On
    /// [`Disposition::Keep`], do nothing at all.
    ///
    /// Keeping used to remove the worktree and spare only the branch, which
    /// contradicted its own documentation and was caught by the first real run:
    /// the CLI printed "kept — `axio session diff <id>` to read it" and the diff
    /// then failed, because the directory it named had just been deleted.
    /// Reviewing a session's work means opening the checkout it worked in, so
    /// keeping has to keep it.
    ///
    /// Discarding refuses while the branch holds commits the repository does
    /// not: closing a window is not consent to delete work, and the message
    /// says how many commits are at stake so the choice is an informed one.
    pub async fn close(&self, disposition: Disposition) -> Result<()> {
        let Some(branch) = &self.branch else {
            return Ok(());
        };
        if disposition == Disposition::Keep {
            return Ok(());
        }

        let commits = self.unmerged_commits().await.unwrap_or(0);
        if commits > 0 {
            return Err(SupervisorError::Git {
                args: format!("branch -D {branch}"),
                message: format!(
                    "`{branch}` has {commits} commit(s) that are nowhere else; \
                     keep it, or delete the branch yourself once you have looked"
                ),
            });
        }

        // `--force` covers a dirty tree, which is the normal state of a session
        // that ended mid-edit rather than an anomaly.
        match git::run(
            &self.repo,
            &[
                "worktree",
                "remove",
                "--force",
                &self.path.to_string_lossy(),
            ],
        )
        .await
        {
            Ok(_) => {}
            // A directory somebody already deleted by hand leaves git holding a
            // registration for it. Pruning is the documented repair, and having
            // to do it manually is a worse answer than doing it here.
            Err(SupervisorError::Git { .. }) => {
                let _ = git::run(&self.repo, &["worktree", "prune"]).await;
            }
            Err(other) => return Err(other),
        }

        if disposition == Disposition::Discard {
            git::run(&self.repo, &["branch", "-D", branch]).await?;
        }
        Ok(())
    }
}
