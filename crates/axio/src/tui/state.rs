//! What the surface is, as opposed to what it does.
//!
//! The mode it is in and the state it holds. Split from `mod` because that file
//! reached the width limit, and these are what a reader wants first: the loop
//! only makes sense once the states it moves between are named.

use super::*;

/// What the interface is doing, and therefore what a keystroke means.
pub(super) enum Mode {
    Idle,
    Running,
    Approving(Box<ApprovalRequest>, tokio::sync::oneshot::Sender<Decision>),
    /// Storing a credential. Its own mode rather than a flag, because while it
    /// is on every keystroke belongs to it — a character typed here must not
    /// also reach the composer, where it would be drawn.
    LoggingIn(Login),
    /// Choosing a model from what the provider listed.
    PickingModel(Picker),
}

pub struct Tui {
    /// Sessions running beside this one, each in its own worktree.
    ///
    /// `None` when nothing could open the index — a surface that cannot
    /// supervise still has to work, so `/new` reports why rather than the whole
    /// interface refusing to start over a directory it could not read.
    pub(super) supervisor: Option<std::sync::Arc<axio_supervisor::Supervisor>>,
    /// How a background session reports back. Cloned into every spawned turn.
    pub(super) notes: tokio::sync::mpsc::UnboundedSender<super::background::Note>,
    /// The repository `/new` starts sessions in. Resolved once at startup,
    /// because the working directory is where the surface was launched and
    /// nothing later moves it.
    pub(super) repo: std::path::PathBuf,
    /// Call ids whose terminal status has been printed, so a status reached
    /// through both `ItemUpdated` and `ItemCompleted` prints one line.
    pub(super) reported: std::collections::HashSet<String>,
    pub(super) composer: Composer,
    /// Where the highlight is in the slash menu. Kept across openings so the
    /// menu is not the only widget on screen with amnesia; the filter clamps
    /// it, so a stale index can never select something that is not shown.
    pub(super) menu: Menu,
    pub(super) mode: Mode,
    /// The part of the streaming message that has no newline after it yet, so
    /// the tail can be shown live before it can be rendered and committed.
    pub(super) live: String,
    /// Markdown state carried across the lines of the message being streamed —
    /// an open code fence is the only thing that outlives a line.
    pub(super) md: markdown::Renderer,
    /// Whether any of the current message has already reached scrollback, which
    /// is what decides whether a discarded stream has to warn that the text
    /// above may repeat.
    pub(super) flushed: bool,
    /// Whether the renderer has already been fed this message's deltas. It is
    /// not the same question as `flushed`: a message whose lines are all still
    /// held back — a table, whose columns are only known at its last row — has
    /// been consumed without anything reaching the screen, and rendering the
    /// completed item again would draw it twice.
    pub(super) streamed: bool,
    /// What the turn is doing right now — thinking, writing, or the subject of
    /// the tool it is waiting on.
    pub(super) status: String,
    /// The turn's cumulative usage, kept apart from `status` so a token count
    /// arriving does not overwrite what the turn was doing.
    pub(super) tokens: Option<(u64, u64)>,
    pub(super) interrupt_armed: bool,
    pub(super) model: String,
    /// What `/status` reports besides the model: provider, endpoint, where the
    /// credential came from, the permission rules, the workspace root.
    ///
    /// Rendered once, before the surface starts, because none of it can change
    /// while a session runs — the model is the one part that can, and it is
    /// read live from the field above rather than kept here.
    pub(super) facts: Vec<String>,
    /// How to build a provider by name, so the interface can move a session to
    /// another one without knowing where credentials live.
    pub(super) factory: crate::provider::Factory,
    /// Every provider and whether it is usable, for the picker's first stage.
    pub(super) offers: Vec<Offer>,
    /// A provider and model that were adopted but have not yet answered.
    ///
    /// Held rather than written at the moment of choosing: the name is not
    /// checked until the next request, so saving then would make a typo the
    /// default and every later session would start broken.
    pub(super) unproven_default: Option<(String, String)>,
    /// Chosen in the first stage and applied with the model in the second.
    /// Held rather than applied immediately, because a provider changed on its
    /// own leaves the session naming a model its endpoint has never heard of.
    pub(super) pending_provider: Option<String>,
    /// When the running turn started, which is what the status counts up from.
    /// A turn that has produced nothing for thirty seconds looks identical to a
    /// hung one unless something on screen is still moving.
    pub(super) started: Option<Instant>,
}
