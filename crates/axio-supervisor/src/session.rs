//! One session, running on its own task.
//!
//! `Agent::run_turn` takes `&mut self`, which is the right shape for one
//! conversation and the wrong one for five: a shared agent behind a lock would
//! serialise every session behind whichever is currently talking to a model.
//!
//! So each session owns its agent on a dedicated task and is spoken to through
//! a channel. Cancellation deliberately does **not** travel that channel — the
//! task is inside `run_turn` and would not read a message until the turn it is
//! meant to interrupt had already finished. It travels through a token the
//! handle holds instead, which is the one thing that can reach a turn already
//! in flight.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axio_core::Agent;
use axio_core::protocol::{SessionId, TurnOutcome};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::error::{Result, SupervisorError};
use crate::project::ProjectId;
use crate::worktree::Checkout;

pub(crate) enum SessionCommand {
    Turn {
        prompt: String,
        reply: oneshot::Sender<TurnOutcome>,
    },
    Close {
        reply: oneshot::Sender<()>,
    },
}

/// What a session is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Running,
}

/// A live session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub session: SessionId,
    pub project: ProjectId,
    pub checkout: Checkout,
    commands: mpsc::Sender<SessionCommand>,
    /// Replaced at the start of every turn. Held behind a lock rather than sent
    /// down the command channel because a turn already running is exactly what
    /// it has to reach.
    cancel: Arc<Mutex<CancellationToken>>,
    running: Arc<AtomicBool>,
}

impl SessionHandle {
    pub fn status(&self) -> SessionStatus {
        if self.running.load(Ordering::SeqCst) {
            SessionStatus::Running
        } else {
            SessionStatus::Idle
        }
    }

    /// Run one turn and wait for its outcome.
    ///
    /// Queued behind any turn already running on this session, which is what
    /// makes a session a conversation rather than a race. Other sessions are
    /// unaffected — that is the whole point of a task each.
    pub async fn turn(&self, prompt: impl Into<String>) -> Result<TurnOutcome> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(SessionCommand::Turn {
                prompt: prompt.into(),
                reply: tx,
            })
            .await
            .map_err(|_| SupervisorError::SessionGone(self.session))?;
        rx.await
            .map_err(|_| SupervisorError::SessionGone(self.session))
    }

    /// Interrupt the turn in flight, if there is one.
    ///
    /// Idle is not an error: cancelling something that already finished is the
    /// normal outcome of a person clicking stop as a turn ends.
    pub fn cancel(&self) {
        self.cancel
            .lock()
            .expect("the cancellation lock is never held across an await")
            .cancel();
    }

    pub(crate) async fn close(&self) -> Result<()> {
        // Cancel first. The task is inside `run_turn` for as long as a turn is
        // running and will not read the close message until it is out.
        self.cancel();
        let (tx, rx) = oneshot::channel();
        if self
            .commands
            .send(SessionCommand::Close { reply: tx })
            .await
            .is_err()
        {
            // The task is already gone, which is the state close was asking for.
            return Ok(());
        }
        let _ = rx.await;
        Ok(())
    }
}

/// Start the task that owns an agent, and hand back the way to talk to it.
pub(crate) fn spawn(
    mut agent: Agent,
    project: ProjectId,
    checkout: Checkout,
    resumed: bool,
    notices: Vec<axio_core::protocol::Notice>,
) -> SessionHandle {
    let session = agent.session_id();
    // Before the first turn and before the task exists, so `SessionStarted` is
    // the first event any consumer sees — the promise `--json` makes, kept for
    // a stream that now carries several sessions at once.
    agent.announce(resumed, notices);

    let (tx, mut rx) = mpsc::channel::<SessionCommand>(16);
    let cancel = Arc::new(Mutex::new(CancellationToken::new()));
    let running = Arc::new(AtomicBool::new(false));

    let task_cancel = Arc::clone(&cancel);
    let task_running = Arc::clone(&running);
    tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            match command {
                SessionCommand::Turn { prompt, reply } => {
                    let token = CancellationToken::new();
                    *task_cancel
                        .lock()
                        .expect("the cancellation lock is never held across an await") =
                        token.clone();
                    task_running.store(true, Ordering::SeqCst);
                    let outcome = agent.run_turn(prompt, token).await;
                    task_running.store(false, Ordering::SeqCst);
                    // A caller that stopped waiting is not a failure; the turn
                    // still happened and is still in the transcript.
                    let _ = reply.send(outcome);
                }
                SessionCommand::Close { reply } => {
                    let _ = reply.send(());
                    break;
                }
            }
        }
    });

    SessionHandle {
        session,
        project,
        checkout,
        commands: tx,
        cancel,
        running,
    }
}
