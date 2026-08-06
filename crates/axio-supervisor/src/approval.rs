//! One queue of questions, across every session.
//!
//! With one session, "ask before each write" is a prompt. With five it is the
//! thing you spend the day on, so the questions are pooled rather than owned by
//! whichever window happens to be focused.
//!
//! The decision still returns through [`Approver`], not through the event
//! stream — that is the constraint `axio_core::approver` exists to state, and
//! it is what lets a turn await an answer without deadlocking against the task
//! that would give it. What changes here is only *who* is asked: a oneshot per
//! question, resolved by whoever is looking at the queue.

use std::collections::BTreeMap;
use std::sync::Mutex;

use axio_core::approver::Approver;
use axio_core::protocol::{ApprovalId, ApprovalRequest, Decision, SessionId};
use tokio::sync::oneshot;

use crate::project::ProjectId;

/// A question waiting for an answer.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub id: ApprovalId,
    pub session: SessionId,
    pub project: ProjectId,
    pub request: ApprovalRequest,
    /// When it was asked, so a queue can show the oldest first and a caller can
    /// tell a question that just arrived from one that has been sitting there.
    pub at_ms: u64,
}

struct Waiting {
    pending: PendingApproval,
    reply: oneshot::Sender<Decision>,
}

/// Every unanswered question, from every session.
#[derive(Default)]
pub struct ApprovalQueue {
    /// `BTreeMap` keyed by a ULID: iteration is oldest-first for free, which is
    /// the order a review queue wants and the order a `HashMap` would scramble
    /// differently on every read.
    waiting: Mutex<BTreeMap<ApprovalId, Waiting>>,
    /// Set when nobody is left to answer.
    ///
    /// Needed because the queue cannot simply be dropped: every session's
    /// approver holds an `Arc` to it, so it outlives the supervisor by
    /// construction, and session tasks are detached and keep running. Without
    /// this flag, dropping a supervisor leaves any turn that asks a question
    /// waiting for an answer that can never come — a hang with no owner.
    closed: std::sync::atomic::AtomicBool,
}

impl ApprovalQueue {
    /// Everything outstanding, oldest first.
    pub fn pending(&self) -> Vec<PendingApproval> {
        self.waiting
            .lock()
            .expect("the approval queue lock is never held across an await")
            .values()
            .map(|w| w.pending.clone())
            .collect()
    }

    pub fn pending_for(&self, session: SessionId) -> Vec<PendingApproval> {
        self.pending()
            .into_iter()
            .filter(|p| p.session == session)
            .collect()
    }

    /// Answer one. `false` if it was already answered, or never existed —
    /// a double-click on Approve is not an error worth surfacing.
    pub fn resolve(&self, id: ApprovalId, decision: Decision) -> bool {
        let Some(waiting) = self
            .waiting
            .lock()
            .expect("the approval queue lock is never held across an await")
            .remove(&id)
        else {
            return false;
        };
        // The receiver is gone if the turn was cancelled while we held the
        // question. Nothing to report: the turn already ended.
        waiting.reply.send(decision).is_ok()
    }

    /// Answer everything a session is holding, because the session is going
    /// away.
    ///
    /// Dropping the senders instead would work — `decide` treats a dropped
    /// sender as a denial — but only by accident. Saying it explicitly means
    /// the model gets the feedback below rather than the generic one.
    pub(crate) fn close_session(&self, session: SessionId) {
        let ids: Vec<ApprovalId> = self
            .pending_for(session)
            .into_iter()
            .map(|p| p.id)
            .collect();
        for id in ids {
            self.resolve(
                id,
                Decision::Deny {
                    feedback: Some(
                        "denied: this session was closed while the request was waiting. \
                         The decision is final; do not retry."
                            .into(),
                    ),
                },
            );
        }
    }

    /// Answer everything and refuse to take any more questions.
    ///
    /// Called when the supervisor goes away. Sessions are detached tasks that
    /// outlive it, so this is what stops one of them parking forever inside
    /// `decide`.
    pub(crate) fn shutdown(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        let ids: Vec<ApprovalId> = self.pending().into_iter().map(|p| p.id).collect();
        for id in ids {
            self.resolve(id, unanswerable());
        }
    }

    /// `None` once the queue is closed: there is nobody to ask, so the caller
    /// must decide rather than wait.
    fn register(
        &self,
        session: SessionId,
        project: ProjectId,
        request: &ApprovalRequest,
    ) -> Option<oneshot::Receiver<Decision>> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        let pending = PendingApproval {
            id: request.id,
            session,
            project,
            request: request.clone(),
            at_ms: now_ms(),
        };
        self.waiting
            .lock()
            .expect("the approval queue lock is never held across an await")
            .insert(request.id, Waiting { pending, reply: tx });
        Some(rx)
    }
}

/// The answer when there is nobody to ask.
///
/// Written for the only party that reads it. Telling a model to "try again
/// later" produces the retry loop `NonInteractive::deny` documents — the same
/// write attempted eleven times in one turn — so it says the decision is final.
fn unanswerable() -> Decision {
    Decision::Deny {
        feedback: Some(
            "denied: nobody is available to approve this. The decision is final for this \
             session and retrying — with different arguments, through a shell, or at all — \
             will be denied again. Continue without it and report plainly what you could \
             not do."
                .into(),
        ),
    }
}

/// Which session an approver belongs to, filled in once the agent exists.
///
/// The ordering problem is real and small: an agent mints its own `SessionId`,
/// and it cannot be built without an approver, so the approver necessarily
/// exists first. A `OnceLock` rather than a `Mutex<Option<_>>` because it is
/// written exactly once, before the session's task is spawned — and no approval
/// can be requested before a turn, so it is always set by the time it is read.
#[derive(Clone, Default)]
pub struct SessionSlot(std::sync::Arc<std::sync::OnceLock<SessionId>>);

impl SessionSlot {
    pub(crate) fn set(&self, session: SessionId) {
        let _ = self.0.set(session);
    }

    fn get(&self) -> SessionId {
        // Nil only if a decision were asked for before the session was spawned,
        // which the ordering above rules out. Reporting nil beats panicking in
        // an approver: the question still reaches the queue and can still be
        // answered.
        self.0.get().copied().unwrap_or(SessionId::nil())
    }
}

/// The [`Approver`] a supervised session is built with.
///
/// One per session, so it knows which session is asking without the queue
/// having to be told twice.
pub struct QueueApprover {
    queue: std::sync::Arc<ApprovalQueue>,
    session: SessionSlot,
    project: ProjectId,
}

impl QueueApprover {
    pub(crate) fn new(
        queue: std::sync::Arc<ApprovalQueue>,
        session: SessionSlot,
        project: ProjectId,
    ) -> Self {
        Self {
            queue,
            session,
            project,
        }
    }
}

#[async_trait::async_trait]
impl Approver for QueueApprover {
    async fn decide(&self, req: &ApprovalRequest) -> Decision {
        // Registered, then awaited — the lock is released before the await, or
        // one pending question would stop every other session from asking one.
        let Some(rx) = self
            .queue
            .register(self.session.get(), self.project.clone(), req)
        else {
            // The queue is closed. Nobody will ever answer, so this has to be a
            // decision rather than a wait: a turn parked in `decide` never
            // ends, and the task it is on is detached.
            return unanswerable();
        };
        match rx.await {
            Ok(decision) => decision,
            // The sender went away without answering — the queue was emptied
            // under us. Same reasoning, same answer.
            Err(_) => unanswerable(),
        }
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_core::tool::Effects;
    use std::sync::Arc;

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalId::generate(),
            call_id: "toolu_1".into(),
            tool: "write".into(),
            subject: "write:src/lib.rs".into(),
            effects: Effects {
                reads: true,
                writes: true,
                executes: false,
                network: false,
            },
            preview: None,
            reason: "writes are not auto-approved".into(),
        }
    }

    fn approver(queue: &Arc<ApprovalQueue>) -> (QueueApprover, SessionId) {
        let session = SessionId::generate();
        let slot = SessionSlot::default();
        slot.set(session);
        (
            QueueApprover::new(
                Arc::clone(queue),
                slot,
                ProjectId::of(std::path::Path::new("/w")),
            ),
            session,
        )
    }

    #[tokio::test]
    async fn a_question_waits_until_it_is_answered() {
        let queue = Arc::new(ApprovalQueue::default());
        let (approver, _) = approver(&queue);
        let req = request();
        let id = req.id;

        let asking = tokio::spawn(async move { approver.decide(&req).await });
        // The question has to appear before anyone can answer it.
        while queue.pending().is_empty() {
            tokio::task::yield_now().await;
        }
        assert!(queue.resolve(id, Decision::AllowSession));
        assert_eq!(asking.await.unwrap(), Decision::AllowSession);
        assert!(queue.pending().is_empty(), "answered questions leave");
    }

    /// The deadlock this whole design is arranged to avoid: one unanswered
    /// question must not stop another session from asking one.
    #[tokio::test]
    async fn one_waiting_question_does_not_block_another_session() {
        let queue = Arc::new(ApprovalQueue::default());
        let (first, _) = approver(&queue);
        let (second, _) = approver(&queue);
        let (a, b) = (request(), request());
        let b_id = b.id;

        let _stuck = tokio::spawn(async move { first.decide(&a).await });
        let moving = tokio::spawn(async move { second.decide(&b).await });

        while queue.pending().len() < 2 {
            tokio::task::yield_now().await;
        }
        queue.resolve(b_id, Decision::Allow);
        assert_eq!(moving.await.unwrap(), Decision::Allow);
        assert_eq!(queue.pending().len(), 1, "the first is still waiting");
    }

    #[tokio::test]
    async fn closing_a_session_answers_its_questions_rather_than_stranding_them() {
        let queue = Arc::new(ApprovalQueue::default());
        let (approver, session) = approver(&queue);
        let req = request();

        let asking = tokio::spawn(async move { approver.decide(&req).await });
        while queue.pending().is_empty() {
            tokio::task::yield_now().await;
        }
        queue.close_session(session);

        match asking.await.unwrap() {
            Decision::Deny { feedback } => {
                assert!(feedback.unwrap().contains("closed"));
            }
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    /// A turn parked in `decide` never ends, and session tasks are detached —
    /// so a supervisor going away has to answer, not vanish. The queue cannot
    /// simply be dropped to achieve that: every approver holds an `Arc` to it,
    /// so it outlives the supervisor by construction.
    #[tokio::test]
    async fn a_question_asked_after_shutdown_is_denied_rather_than_hanging() {
        let queue = Arc::new(ApprovalQueue::default());
        let (approver, _) = approver(&queue);
        queue.shutdown();

        let decided = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            approver.decide(&request()),
        )
        .await
        .expect("a closed queue must not hang the turn");
        match decided {
            Decision::Deny { feedback } => {
                let feedback = feedback.expect("a model reads this");
                assert!(feedback.contains("final"), "{feedback}");
            }
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    /// And a question already waiting when the lights go out is answered too.
    #[tokio::test]
    async fn shutdown_answers_the_questions_already_waiting() {
        let queue = Arc::new(ApprovalQueue::default());
        let (approver, _) = approver(&queue);
        let req = request();

        let asking = tokio::spawn(async move { approver.decide(&req).await });
        while queue.pending().is_empty() {
            tokio::task::yield_now().await;
        }
        queue.shutdown();

        let decided = tokio::time::timeout(std::time::Duration::from_millis(500), asking)
            .await
            .expect("shutdown must not strand a waiting turn")
            .unwrap();
        assert!(matches!(decided, Decision::Deny { .. }));
        assert!(queue.pending().is_empty());
    }

    #[test]
    fn answering_the_same_question_twice_is_not_an_error() {
        let queue = ApprovalQueue::default();
        assert!(!queue.resolve(ApprovalId::generate(), Decision::Allow));
    }
}
