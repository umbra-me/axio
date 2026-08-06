//! Repositories and agents to test against.
//!
//! Real git, because every failure this crate exists to handle — a repository
//! with no commits, a branch that already exists, a worktree somebody deleted
//! by hand — is a real git behaviour, and a fake would encode what we assumed
//! git does rather than what it does.
//!
//! The agents are scripted: no network, no credential, milliseconds per test.
//! That is the property `axio-core` protects and the reason [`AgentFactory`]
//! exists at all.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axio_core::scripted::ScriptedProvider;
use axio_core::{Agent, RuntimeConfig, Session};

use crate::factory::{AgentFactory, AgentRequest};

/// A git repository with one commit, in a directory that cleans itself up.
pub struct Repo {
    pub dir: tempfile::TempDir,
}

impl Repo {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path();
        run(path, &["init", "-b", "main"]).await;
        // Set locally: a machine with no global identity cannot commit, and a
        // test that depends on the developer's git config is a test that passes
        // here and fails in CI.
        run(path, &["config", "user.email", "test@axio.invalid"]).await;
        run(path, &["config", "user.name", "axio test"]).await;
        std::fs::write(path.join("README.md"), "seed\n").expect("the seed file");
        run(path, &["add", "."]).await;
        run(path, &["commit", "-m", "seed"]).await;
        Self { dir }
    }

    /// A repository that was initialised and never committed to.
    pub async fn empty() -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        run(dir.path(), &["init", "-b", "main"]).await;
        Self { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub async fn git(&self, args: &[&str]) -> String {
        crate::git::run(self.path(), args)
            .await
            .unwrap_or_else(|e| panic!("git {}: {e}", args.join(" ")))
    }

    /// Whether a branch exists in this repository.
    pub async fn has_branch(&self, branch: &str) -> bool {
        crate::git::branch_exists(self.path(), branch).await
    }
}

async fn run(cwd: &Path, args: &[&str]) {
    crate::git::run(cwd, args)
        .await
        .unwrap_or_else(|e| panic!("git {}: {e}", args.join(" ")));
}

/// Builds agents that answer from a script.
///
/// Records the workspace each agent was built with, so a test can assert what
/// the supervisor handed the factory without reaching inside the agent.
#[derive(Default)]
pub struct ScriptedFactory {
    pub built: Arc<std::sync::Mutex<Vec<PathBuf>>>,
    /// Fail every build with this message, to exercise the cleanup path.
    pub fail_with: Option<String>,
}

impl ScriptedFactory {
    pub fn failing(message: &str) -> Self {
        Self {
            built: Arc::default(),
            fail_with: Some(message.to_owned()),
        }
    }

    pub fn workspaces(&self) -> Vec<PathBuf> {
        self.built.lock().expect("the fixture lock").clone()
    }
}

#[async_trait::async_trait]
impl AgentFactory for ScriptedFactory {
    async fn build(&self, request: AgentRequest) -> Result<Agent, String> {
        if let Some(message) = &self.fail_with {
            return Err(message.clone());
        }
        self.built
            .lock()
            .expect("the fixture lock")
            .push(request.checkout.path.clone());

        Ok(Agent::new(
            Arc::new(ScriptedProvider::say("done")),
            request.approver,
            Session::new(request.checkout.path.clone(), "claude-opus-5"),
            RuntimeConfig::default(),
            vec![],
            request.events,
        ))
    }
}
