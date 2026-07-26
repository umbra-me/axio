//! The one interactive host seam.
//!
//! The decision returns through this trait rather than through the event
//! stream. That is what lets the turn run inline on `&mut self` without
//! deadlocking against the task that would answer it: a pending-approval map
//! consulted by the same `&mut self` that is awaiting inside the turn cannot
//! make progress, because the task that would read the answer never runs.

use crate::protocol::{ApprovalRequest, Decision};

#[async_trait::async_trait]
pub trait Approver: Send + Sync + 'static {
    async fn decide(&self, req: &ApprovalRequest) -> Decision;
}

/// One-shot CLI. Never blocks, never prompts, never assumes a TTY.
///
/// `on_ask` is `Deny` by default and `Allow` under `--yes`. A non-interactive
/// surface that would otherwise hang instead answers immediately, which is why
/// the acceptance test for it is a timeout.
#[derive(Debug, Clone)]
pub struct NonInteractive {
    pub on_ask: Decision,
}

impl NonInteractive {
    /// Deny anything policy could not decide alone.
    ///
    /// The feedback is written for the only party that reads it. It used to say
    /// "re-run in the TUI, or pass `--yes`" — advice addressed to a human, sent
    /// to a model that can do neither. Observed consequences: the same write
    /// attempted eleven times in one turn, an invented `"yes": true` argument on
    /// the tool, and `--yes` appended to the shell command itself, a habit that
    /// then outlived the denials. Saying the decision is final is what stops the
    /// loop; the human half of the message belongs on stderr, where a human is.
    pub fn deny() -> Self {
        Self {
            on_ask: Decision::Deny {
                feedback: Some(
                    "denied: this action requires approval and no one is available to give it. \
                     The decision is final for this session and retrying — with different \
                     arguments, through a shell, or at all — will be denied again. Continue \
                     without it and report plainly what you could not do."
                        .into(),
                ),
            },
        }
    }

    /// `--yes`: allow anything policy could not decide alone.
    pub fn allow() -> Self {
        Self {
            on_ask: Decision::Allow,
        }
    }
}

impl Default for NonInteractive {
    fn default() -> Self {
        Self::deny()
    }
}

#[async_trait::async_trait]
impl Approver for NonInteractive {
    async fn decide(&self, _req: &ApprovalRequest) -> Decision {
        self.on_ask.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ApprovalId;
    use crate::tool::Effects;

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalId::nil(),
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

    #[tokio::test]
    async fn non_interactive_denies_without_blocking() {
        let approver = NonInteractive::deny();
        let decided = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            approver.decide(&request()),
        )
        .await
        .expect("a non-interactive approver must never block");
        assert!(matches!(decided, Decision::Deny { .. }));
    }

    #[tokio::test]
    async fn yes_flag_allows() {
        let approver = NonInteractive::allow();
        assert_eq!(approver.decide(&request()).await, Decision::Allow);
    }

    #[tokio::test]
    async fn is_object_safe() {
        let approver: Box<dyn Approver> = Box::new(NonInteractive::deny());
        assert!(matches!(
            approver.decide(&request()).await,
            Decision::Deny { .. }
        ));
    }
}
