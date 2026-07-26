//! A provider that replays a recorded event list.
//!
//! This is the primary test surface for every loop property: microseconds, no
//! I/O, no HTTP stack. It lives in this crate rather than in `axio-provider` so
//! that `cargo test -p axio-core` can exercise the whole loop while the crate
//! stays free of a transport dependency. Gated behind the `testing` feature so
//! it never reaches a release binary.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::provider::{
    BoxStream, Effort, ModelInfo, ModelRequest, Provider, ProviderError, StreamEvent,
};

/// One scripted turn: either a stream of events or a failure.
pub enum Script {
    Events(Vec<StreamEvent>),
    Error(ProviderError),
    /// Emit some events, then fail — the mid-stream failure case that a retry
    /// must not double-print.
    PartialThenError(Vec<StreamEvent>, ProviderError),
}

/// Replays one `Script` per call to `stream`, in order.
pub struct ScriptedProvider {
    scripts: Mutex<std::collections::VecDeque<Script>>,
    /// Every request this provider was asked for, in order — so a test can
    /// assert on the exact bytes that would have gone out.
    pub seen: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ScriptedProvider {
    pub fn new(scripts: impl IntoIterator<Item = Script>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into_iter().collect()),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The common case: one turn that streams text and ends.
    pub fn say(text: &str) -> Self {
        Self::new([Script::Events(vec![
            StreamEvent::MessageStart {
                id: "msg_scripted".into(),
            },
            StreamEvent::BlockStart {
                index: 0,
                kind: crate::provider::BlockKind::Text,
            },
            StreamEvent::TextDelta {
                index: 0,
                text: text.to_owned(),
            },
            StreamEvent::BlockEnd { index: 0 },
            StreamEvent::Done {
                stop: crate::provider::StopReason::EndTurn,
            },
        ])])
    }

    pub fn requests(&self) -> Vec<ModelRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    fn model_info(&self, _model: &str) -> ModelInfo {
        ModelInfo {
            context_window: 1_000_000,
            max_output_tokens: 64_000,
            input_price: 5.0,
            output_price: 25.0,
            cache_read_price: 0.5,
            cache_write_price: 6.25,
        }
    }

    async fn stream(
        &self,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        self.seen.lock().unwrap().push(req);

        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }

        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Script::Error(ProviderError::Transport(
                crate::redact::Redacted::new("scripted provider ran out of scripts"),
            )));

        let items: Vec<Result<StreamEvent, ProviderError>> = match script {
            Script::Events(events) => events.into_iter().map(Ok).collect(),
            Script::Error(e) => return Err(e),
            Script::PartialThenError(events, e) => events
                .into_iter()
                .map(Ok)
                .chain(std::iter::once(Err(e)))
                .collect(),
        };

        Ok(Box::pin(ScriptStream {
            items: items.into(),
            cancel,
        }))
    }
}

struct ScriptStream {
    items: std::collections::VecDeque<Result<StreamEvent, ProviderError>>,
    cancel: CancellationToken,
}

impl futures_core::Stream for ScriptStream {
    type Item = Result<StreamEvent, ProviderError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.cancel.is_cancelled() {
            return std::task::Poll::Ready(Some(Err(ProviderError::Cancelled)));
        }
        std::task::Poll::Ready(self.items.pop_front())
    }
}

/// Convenience for assembling a scripted request in tests.
pub fn scripted_request(model: &str, effort: Effort) -> ModelRequest {
    let mut req = ModelRequest::new(model);
    req.effort = effort;
    req
}
