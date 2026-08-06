//! Who builds the agents.
//!
//! Not this crate, deliberately. Building one means choosing a provider,
//! registering tools, resolving a policy and writing a system prompt — and
//! every one of those is a decision the *surface* has already made. A
//! supervisor that made them again would be a second copy of `prepare()`,
//! which is precisely how two surfaces end up with permission rules that differ
//! by accident.
//!
//! So the supervisor manages lifecycle and the caller supplies agents. Three
//! things fall out of that. This crate links no HTTP stack and no tool
//! implementations. The CLI and the desktop app can drive the same supervisor
//! with the same wiring, which is what keeps them one product rather than two.
//! And its own tests run against a scripted provider, in milliseconds, with no
//! network — the same property `axio-core` protects.

use std::sync::Arc;

use axio_core::Agent;
use axio_core::approver::Approver;
use axio_core::protocol::{Event, SessionId};
use tokio::sync::mpsc;

use crate::project::Project;
use crate::worktree::Checkout;

/// Everything the supervisor has decided before an agent exists.
///
/// The approver and the event sender are not suggestions: an agent built with
/// a different approver leaves its questions out of the queue, and one built
/// with a different sender leaves its events out of the stream. A factory that
/// ignores them produces a session the supervisor cannot see.
pub struct AgentRequest {
    pub project: Project,
    /// Where the agent works. This must become the session's `cwd`, because
    /// that is what `Workspace` confines every tool to.
    pub checkout: Checkout,
    pub approver: Arc<dyn Approver>,
    pub events: mpsc::UnboundedSender<Event>,
    /// Continue an existing session rather than starting one.
    pub resume: Option<SessionId>,
    /// The first prompt, where there is one, for the session header's label.
    pub label: Option<String>,
}

#[async_trait::async_trait]
pub trait AgentFactory: Send + Sync + 'static {
    /// Build a configured agent rooted at `request.checkout.path`.
    ///
    /// The error is a message rather than a type: everything that can fail here
    /// belongs to the caller's own configuration — no credential, an unreachable
    /// endpoint, an unreadable session — and it already knows how to say so
    /// better than this crate could.
    async fn build(&self, request: AgentRequest) -> Result<Agent, String>;
}
