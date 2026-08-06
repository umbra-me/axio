//! Many agent sessions at once, across many repositories.
//!
//! `axio-core` runs one conversation. This crate runs a lot of them: each on
//! its own task, each in its own git worktree, all reporting into one event
//! stream and one queue of approvals.
//!
//! It is a library and not a surface. The CLI and the desktop app drive the
//! same supervisor with the same wiring — the app is a way of looking at this,
//! not a place where extra capability lives. That is enforced by shape rather
//! than by intention: agents arrive through [`AgentFactory`], so this crate
//! links no HTTP stack and registers no tools, and there is nothing here for a
//! surface to have privileged access to.
//!
//! Four things hold it up.
//!
//! 1. **Isolation is the default and never a fallback.** A session gets a
//!    worktree and a branch; if that fails, starting fails. Silently writing to
//!    the checkout someone is using is the one outcome nobody would notice
//!    until two agents had overwritten each other. [`Isolation::Direct`] exists
//!    and is chosen, never fallen back to.
//! 2. **Nothing here is reachable by a tool.** Worktrees, branches, the index
//!    and the queue are host state. `ToolCx` stays closed at five fields, which
//!    is what keeps every tool working identically in a one-shot CLI run — the
//!    property the predecessor project lost when it grew twelve host bridges
//!    and a third of its tools became desktop-only by construction.
//! 3. **The decision still returns through `Approver`.** Pooling the questions
//!    changes who answers, not how the answer travels. See [`approval`].
//! 4. **One session per task, cancellation out of band.** `run_turn` takes
//!    `&mut self`, so sessions cannot share an agent; and a cancellation sent
//!    down the same channel as the work would not be read until the turn it was
//!    meant to interrupt had finished. See [`session`].
//!
//! Landing work is deliberately absent. Merging, pull requests and
//! cherry-picking are workflows, and a supervisor that picked one would be
//! wrong for the other two. What is provided is the branch name, the status and
//! the diff — everything needed to land it whichever way a caller lands things.

#![forbid(unsafe_code)]

pub mod approval;
mod error;
mod factory;
#[cfg(test)]
mod fixture;
mod git;
pub mod index;
pub mod project;
pub mod session;
mod supervisor;
pub mod worktree;

pub use approval::{ApprovalQueue, PendingApproval, QueueApprover};
pub use error::{Result, SupervisorError};
pub use factory::{AgentFactory, AgentRequest};
pub use index::{IndexEntry, SessionIndex};
pub use project::{Project, ProjectId, Projects};
pub use session::{SessionHandle, SessionStatus};
pub use supervisor::{StartOptions, SupervisedEvent, Supervisor, SupervisorConfig};
pub use worktree::{Checkout, Disposition, Isolation};
