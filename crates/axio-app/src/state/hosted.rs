//! Placing another agent's tool, and what this machine has to offer it.
//!
//! `Hosted` spawns where it is told; deciding where is the state's job,
//! because only the state has the supervisor that cuts worktrees.

use std::path::Path;

use axio_supervisor::Isolation as SupervisorIsolation;

use super::AppState;
use crate::hosted::{HostedView, Place, StartHostedInput};
use crate::model::{AppError, Isolation, ProviderView};

impl AppState {
    /// Start another agent's tool in a terminal, somewhere to work.
    ///
    /// The place is decided here because only the state has the supervisor:
    /// by default the agent gets a worktree of its own on its own branch, cut
    /// the same way a session's is, so several agents on one repository never
    /// share a checkout. `Direct` is a choice the caller makes. The worktree
    /// is kept when the terminal ends — what the agent did is a branch to
    /// read, land or delete, exactly as a session's is.
    ///
    /// The agent's configured arguments come first, then what this launch
    /// asked for, so a launch can add to a default without having to repeat it.
    pub async fn start_hosted(
        &self,
        mut input: StartHostedInput,
        on_activity: impl Fn(String) + Send + 'static,
    ) -> Result<HostedView, AppError> {
        if let Some(settings) = &self.settings
            && let Ok(app) = settings.load()
            && let Some(agent) = app.agents.get(&input.harness)
            && !agent.args.trim().is_empty()
        {
            input.args = format!("{} {}", agent.args.trim(), input.args)
                .trim()
                .to_owned();
        }

        if input.harness == "axio"
            && let Some(settings) = &self.settings
            && let Ok(app) = settings.load()
            && let Some(agent) = app.agents.get("axio")
            && !agent.local_profile.trim().is_empty()
        {
            let profile = agent.local_profile.trim();
            if !profile
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            {
                return Err(AppError::Supervisor(
                    "local profile names use letters, digits, hyphens and underscores".into(),
                ));
            }
            input.args = format!("local {profile} {}", input.args);
        }

        let isolation = input.isolation.unwrap_or(Isolation::Worktree);
        let place = match (isolation, self.supervisor()) {
            (Isolation::Worktree, Ok(supervisor)) => {
                let project = supervisor
                    .open_project(Path::new(&input.cwd))
                    .await
                    .map_err(|e| match e {
                        axio_supervisor::SupervisorError::NotARepository(_) => {
                            AppError::NoRepository(e.to_string())
                        }
                        other => AppError::Supervisor(other.to_string()),
                    })?;
                let checkout = supervisor
                    .checkout(&project, Some(SupervisorIsolation::Worktree))
                    .await
                    .map_err(|e| AppError::Supervisor(e.to_string()))?;
                Place {
                    cwd: checkout.path,
                    repo: checkout.repo,
                    branch: checkout.branch,
                }
            }
            // No supervisor means no worktree can be cut. That is an error
            // for a session; for a terminal, which was going to run another
            // tool as itself anyway, it is a downgrade the caller asked about
            // — so it is refused the same way, and the caller may say `Direct`.
            (Isolation::Worktree, Err(e)) => return Err(e),
            (Isolation::Direct, _) => {
                let cwd = std::path::PathBuf::from(&input.cwd);
                Place {
                    repo: cwd.clone(),
                    cwd,
                    branch: None,
                }
            }
        };
        if input.transport.as_deref() == Some("app") {
            return self.hosted.start_app(input, place, on_activity).await;
        }
        self.hosted.start_with_signal(input, place, on_activity)
    }

    /// Bring a stopped or remembered terminal back where it was. The pane
    /// size is sent for the reason `StartHostedInput` gives: the tool paints
    /// its opening screen at the size it is given, and that paint stays.
    pub async fn resume_hosted(
        &self,
        id: &str,
        rows: Option<u16>,
        cols: Option<u16>,
        on_activity: impl Fn(String) + Send + 'static,
    ) -> Result<HostedView, AppError> {
        self.hosted
            .resume_with_signal(id, rows, cols, on_activity)
            .await
    }

    /// Every provider axio knows, and whether this machine has a credential
    /// for it — the question the opening pane asks before offering to start.
    pub fn providers(&self, home: &Path) -> Vec<ProviderView> {
        let env: Vec<(String, String)> = std::env::vars().collect();
        axio_core::auth::PROVIDERS
            .iter()
            .map(|name| ProviderView {
                name: (*name).to_owned(),
                ready: axio_core::auth::resolve(name, home, &env).is_some(),
            })
            .collect()
    }
}
