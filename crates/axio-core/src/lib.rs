//! The abstract machine: protocol, traits, and (from M2) the turn loop.
//!
//! This crate links no HTTP or TLS stack, no terminal library, no filesystem
//! walker, and no subprocess machinery. That is not an aesthetic preference —
//! it is what lets its test suite exercise the whole loop against fakes in
//! under two seconds, and it is asserted by `scripts/deps.sh` in CI.
//!
//! Three invariants hold everything else up:
//!
//! 1. [`tool::Tool::run`] is the only execution path, and approval is a
//!    pre-flight over a [`tool::Plan`], never an interception by tool name.
//! 2. [`tool::ToolCx`] is closed: five concrete fields, none optional, none
//!    `dyn`. A tool that needs more does not ship.
//! 3. Surfaces consume a stream of [`protocol::Event`] and supply an
//!    [`approver::Approver`]. Nothing else crosses the boundary.

#![forbid(unsafe_code)]

pub mod agent;
pub mod approver;
pub mod compact;
pub mod config;
pub mod policy;
pub mod protocol;
pub mod provider;
pub mod record;
pub mod redact;
pub mod session;
pub mod tool;
pub mod truncate;

#[cfg(any(test, feature = "testing"))]
pub mod scripted;

pub use agent::{Agent, RuntimeConfig};
pub use approver::{Approver, NonInteractive};
pub use protocol::{
    ApprovalRequest, Decision, Delta, Event, EventKind, Item, ItemBody, Notice, NoticeLevel,
    PROTOCOL_VERSION, Preview, ToolStatus, TurnOutcome, Usage,
};
pub use provider::{Effort, ModelRequest, Provider, ProviderError, StopReason, StreamEvent};
pub use redact::{Redacted, register_secret};
pub use session::Session;
pub use tool::{Effects, Plan, Tool, ToolCx, ToolError, ToolOutput, Workspace};
