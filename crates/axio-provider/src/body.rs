//! The tail every transport shares: classify the response status, then drive
//! one SSE body until its decoder is done.
//!
//! The three transports differed only in which decoder they held, so
//! cancellation, the finished flag and truncation-vs-error each existed in
//! three copies of the same `poll_next`. A fix to any of them could be applied
//! twice, compile, and still look complete. The decoder is the only thing that
//! varies between transports, so it is the only thing they supply.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use axio_core::provider::{BoxStream, ProviderError, StreamEvent};
use axio_core::redact::Redacted;
use futures_core::Stream;
use tokio_util::sync::CancellationToken;

/// A transport's SSE decoder, as the shared body needs to see it.
///
/// Both methods hand back whatever frames completed; a decoder holding a
/// partial frame returns nothing and keeps it.
pub(crate) trait EventDecoder: Send + Unpin + 'static {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, ProviderError>;

    /// Called once at end of stream. A frame still buffered here is a
    /// truncation, and each decoder is responsible for surfacing it as
    /// `Truncated` rather than as a decode error.
    fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError>;
}

/// Reject a non-2xx response, carrying `retry-after` into the classification.
///
/// The body is read to completion first: the error type is chosen from what
/// the endpoint said, not from the status alone.
pub(crate) async fn check_status(
    response: reqwest::Response,
) -> Result<reqwest::Response, ProviderError> {
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.unwrap_or_default();
    Err(crate::anthropic::classify(
        status,
        retry_after.as_deref(),
        &body,
    ))
}

/// Drive `decoder` over the response body until it ends, is cancelled, or fails.
pub(crate) fn events<D: EventDecoder>(
    response: reqwest::Response,
    decoder: D,
    cancel: CancellationToken,
) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
    Box::pin(Body {
        inner: Box::pin(response.bytes_stream()),
        decoder,
        pending: VecDeque::new(),
        cancel,
        finished: false,
    })
}

struct Body<D> {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
    decoder: D,
    pending: VecDeque<StreamEvent>,
    cancel: CancellationToken,
    finished: bool,
}

impl<D: EventDecoder> Stream for Body<D> {
    type Item = Result<StreamEvent, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            if self.finished {
                return Poll::Ready(None);
            }
            // Checked before the poll, not after: a cancelled turn must not
            // wait on a chunk that may never arrive.
            if self.cancel.is_cancelled() {
                self.finished = true;
                return Poll::Ready(Some(Err(ProviderError::Cancelled)));
            }
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => match self.decoder.push(&chunk) {
                    Ok(events) => self.pending.extend(events),
                    Err(e) => {
                        self.finished = true;
                        return Poll::Ready(Some(Err(e)));
                    }
                },
                Poll::Ready(Some(Err(e))) => {
                    self.finished = true;
                    return Poll::Ready(Some(Err(ProviderError::Transport(Redacted::new(
                        e.to_string(),
                    )))));
                }
                // Not a return: `finish` may still yield the frames it was
                // holding, and those are drained by the next turn of the loop.
                Poll::Ready(None) => {
                    self.finished = true;
                    match self.decoder.finish() {
                        Ok(events) => self.pending.extend(events),
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
