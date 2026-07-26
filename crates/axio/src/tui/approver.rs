//! The interactive half of the approval ladder.
//!
//! The decision travels back through the `Approver` trait rather than through
//! the event stream, and the turn awaits it inline on `&mut self`. So the loop
//! and the interface cannot be the same task: this hands the request across a
//! channel and waits on a reply, which is what lets a human answer without the
//! loop holding a lock on itself.

use axio_core::approver::Approver;
use axio_core::protocol::{ApprovalRequest, Decision};
use tokio::sync::{mpsc, oneshot};

/// One question, and where to put the answer.
pub struct Ask {
    pub request: ApprovalRequest,
    pub reply: oneshot::Sender<Decision>,
}

#[derive(Clone)]
pub struct TuiApprover {
    asks: mpsc::UnboundedSender<Ask>,
}

impl TuiApprover {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<Ask>) {
        let (asks, rx) = mpsc::unbounded_channel();
        (Self { asks }, rx)
    }
}

#[async_trait::async_trait]
impl Approver for TuiApprover {
    async fn decide(&self, req: &ApprovalRequest) -> Decision {
        let (reply, answer) = oneshot::channel();
        let ask = Ask {
            request: req.clone(),
            reply,
        };

        // A closed channel means the interface is gone. Denying is the only
        // safe reading of "nobody is there": the alternative is running a write
        // nobody approved because the window closed.
        if self.asks.send(ask).is_err() {
            return Decision::Deny {
                feedback: Some(
                    "denied: the interface closed before this could be approved. \
                     Do not retry it."
                        .into(),
                ),
            };
        }

        answer.await.unwrap_or(Decision::Deny {
            feedback: Some(
                "denied: no answer was given. Do not retry it; report what you \
                 could not do."
                    .into(),
            ),
        })
    }
}
