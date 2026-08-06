//! The one place a supervised session is assembled.
//!
//! `axio-supervisor` takes agents rather than building them, so that it links
//! no transport and no tools. This is the other half of that seam: the concrete
//! factory, holding everything `prepare` resolves — configuration, provider,
//! policy, protected directories, tools, the sanitised child environment — so a
//! session started by the CLI and a session started by a desktop surface are
//! assembled by the same code rather than by two copies of it.
//!
//! It is `pub` for exactly that reason. Everything else in this crate can stay
//! internal; this is the door.

use std::path::PathBuf;
use std::sync::Arc;

use axio_core::Agent;
use axio_core::config::Resolved;
use axio_core::protocol::Notice;
use axio_core::provider::{Provider, SystemBlock};
use axio_core::record::{Recorder, SessionStore};
use axio_core::session::Session;
use axio_core::tool::ToolEnv;
use axio_supervisor::{AgentFactory, AgentRequest};

use crate::paths::{axio_home, state_dir};
use crate::sessions::recorder_for;
use crate::surfaces::system_prompt;

/// Everything a supervised session needs that is not decided per session.
///
/// Built once from the resolved configuration and shared by every session the
/// supervisor starts, which is what makes "the CLI and the app agree" true by
/// construction rather than by review.
pub struct LocalFactory {
    resolved: Resolved,
    provider: Arc<dyn Provider>,
    /// `--yes`. Carried here rather than read from a flag at build time so a
    /// surface with no flags has to state it.
    unattended: bool,
    /// Record nothing. A supervised session is normally recorded, because the
    /// worktree outlives it and a branch with no transcript is evidence with
    /// half of itself missing.
    ephemeral: bool,
}

impl LocalFactory {
    pub fn new(resolved: Resolved, provider: Arc<dyn Provider>) -> Self {
        Self {
            resolved,
            provider,
            unattended: false,
            ephemeral: false,
        }
    }

    /// Approve anything policy could not decide alone.
    ///
    /// Worth saying out loud on this path: a supervised run is unattended by
    /// default in the sense that nobody is watching *this* session, and the
    /// pooled approval queue is what makes that survivable. Turning it on
    /// removes the queue's reason to exist.
    pub fn unattended(mut self, yes: bool) -> Self {
        self.unattended = yes;
        self
    }

    pub fn ephemeral(mut self, ephemeral: bool) -> Self {
        self.ephemeral = ephemeral;
        self
    }

    /// Where session files live. One store for every surface.
    pub fn session_store() -> SessionStore {
        SessionStore::new(state_dir().join("sessions"))
    }
}

#[async_trait::async_trait]
impl AgentFactory for LocalFactory {
    async fn build(&self, request: AgentRequest) -> Result<Agent, String> {
        let cwd: PathBuf = request.checkout.path.clone();

        let mut cfg = self.resolved.runtime();
        cfg.spill_dir = Some(state_dir().join("outputs"));

        let (policy, _notices) = self.resolved.policy(self.unattended);
        // Same two directories the one-shot and interactive surfaces protect.
        // axio's own home holds the credential file, and a session running from
        // a parent of it would otherwise let `read` hand the key to the model.
        let policy = policy
            .protect(&axio_home())
            .protect(&state_dir().join("sessions"));

        let store = LocalFactory::session_store();
        let mut notices: Vec<Notice> = Vec::new();

        // A resumed session keeps its own id, cwd and model, because a
        // transcript's reasoning is only replayable under the model that minted
        // it. Resuming is the caller's decision and the loader's job; this only
        // has to not invent a second session for one that already exists.
        let session = match request.resume {
            Some(id) => {
                let loaded = axio_core::record::load(&store.path_for(id))
                    .map_err(|e| format!("could not resume {id}: {e}"))?;
                notices.extend(loaded.notices);
                loaded.session
            }
            None => Session::new(cwd.clone(), &cfg.model),
        };
        if request.resume.is_some() {
            cfg = cfg.adopt_model(session.model());
        }

        let recorder = if self.ephemeral {
            Recorder::Ephemeral
        } else {
            recorder_for(
                &store,
                &session,
                request.label.as_deref().unwrap_or_default(),
                request.resume.is_some(),
                &mut notices,
            )
        };

        let mut agent = Agent::new(
            Arc::clone(&self.provider),
            request.approver,
            session,
            cfg,
            vec![SystemBlock {
                text: system_prompt(&cwd),
            }],
            request.events,
        )
        .with_policy(policy)
        .with_recorder(recorder)
        .with_env(ToolEnv {
            vars: axio_tools::proc::child_env(),
        });

        for tool in axio_tools::all() {
            agent.register_tool(tool);
        }
        Ok(agent)
    }
}
