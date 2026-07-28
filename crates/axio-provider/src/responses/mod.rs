//! A provider speaking the Responses dialect, authenticated by sign-in.
//!
//! The third transport, and the one that made the seam an extension point
//! rather than two implementations. It exists because a ChatGPT token is not
//! accepted anywhere else: the subscription endpoint speaks Responses, so
//! reaching it means speaking Responses.
//!
//! What is different here is not only the wire. This is the first provider
//! holding a credential that **expires**, so it renews before a request rather
//! than failing one — and hands the renewed pair to a sink, because a refresh
//! nobody persists is a refresh every process repeats.

mod request;
mod stream;

pub use request::build_body;
pub use stream::ResponsesStream;

use std::pin::Pin;
use std::sync::Arc;

use axio_core::auth::{OAuthTokens, TokenSink};
use axio_core::provider::{
    BoxStream, ModelInfo, ModelRequest, Provider, ProviderError, StreamEvent,
};
use axio_core::redact::Redacted;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::client::transport_error;

/// Where a signed-in session's requests go.
pub const CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex";

/// The provider name this transport answers to.
pub const PROVIDER: &str = "openai-codex";

/// Renew this long before the expiry rather than at it.
///
/// A token with four seconds left is not enough for a turn that streams for
/// thirty, and the failure lands in the middle of an answer.
const SKEW_MS: u64 = 120_000;

/// What to answer when the catalog asks which client version this is.
///
/// It filters on the answer: every model declares a minimum, and axio's own
/// `0.1.0` returns an empty list. axio has no version in that numbering — the
/// question is about a different program — so any value here is a fiction, and
/// this is the one that does not silently hide the whole catalog. What a model
/// says about *itself* is what actually filters the list, and that is read.
const CLIENT_VERSION: &str = "999.0.0";

pub struct CodexProvider {
    http: reqwest::Client,
    /// Behind a lock because a refresh replaces it, and the request path is
    /// shared. Read on every request, written only by a renewal.
    tokens: RwLock<OAuthTokens>,
    sink: Arc<dyn TokenSink>,
    base_url: String,
}

impl CodexProvider {
    pub fn new(
        tokens: OAuthTokens,
        sink: Arc<dyn TokenSink>,
        base_url: Option<String>,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            http: super::oauth::http_client()?,
            tokens: RwLock::new(tokens),
            sink,
            base_url: base_url.unwrap_or_else(|| CODEX_BASE.to_owned()),
        })
    }

    /// A token good for the request about to be made.
    ///
    /// Returns the account id beside it because the endpoint wants both, and
    /// reading them separately across a refresh could pair a fresh token with
    /// the account from the one it replaced.
    async fn usable(&self) -> Result<(String, Option<String>), ProviderError> {
        let now = super::oauth::codex::now_ms();
        {
            let held = self.tokens.read().await;
            if !held.expired(now, SKEW_MS) {
                return Ok((held.access.expose().to_owned(), held.account_id.clone()));
            }
        }

        let mut held = self.tokens.write().await;
        // Checked again with the write lock held. Two requests can both see an
        // expired token, and the second would otherwise spend a second refresh
        // — which some issuers answer by invalidating the first.
        if !held.expired(now, SKEW_MS) {
            return Ok((held.access.expose().to_owned(), held.account_id.clone()));
        }

        let renewed = super::oauth::codex::refresh(&self.http, held.refresh.expose()).await?;
        self.sink.store(PROVIDER, &renewed);
        *held = renewed;
        Ok((held.access.expose().to_owned(), held.account_id.clone()))
    }
}

/// Unpriced, like the other non-Anthropic provider. A subscription does not
/// bill per token, so any figure here would be invented.
pub fn model_info(_model: &str) -> ModelInfo {
    ModelInfo {
        context_window: 272_000,
        max_output_tokens: 128_000,
        input_price: 0.0,
        output_price: 0.0,
        cache_read_price: 0.0,
        cache_write_price: 0.0,
    }
}

#[async_trait::async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> &str {
        PROVIDER
    }

    fn model_info(&self, model: &str) -> ModelInfo {
        model_info(model)
    }

    /// The catalog, asked of the endpoint like every other provider's.
    ///
    /// This was a hardcoded list of three names for exactly one commit, and
    /// every one of them was wrong — the endpoint serves none of them. That is
    /// the argument against compiled-in catalogues in one line: it is not that
    /// they go stale, it is that they can be wrong on the day they are written
    /// and nothing says so until someone picks a name and gets a 404.
    async fn models(&self, cancel: CancellationToken) -> Result<Vec<String>, ProviderError> {
        let (access, account) = self.usable().await?;
        let url = format!("{}/models?client_version={CLIENT_VERSION}", self.base_url);

        let mut request = self.http.get(&url).bearer_auth(&access);
        if let Some(account) = account {
            request = request.header("chatgpt-account-id", account);
        }

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            r = request.send() => r.map_err(|e| transport_error(e, &url))?,
        };

        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(crate::anthropic::classify(status, None, &body));
        }
        crate::catalog::codex_models(&body)
    }

    async fn stream(
        &self,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        use futures_core::Stream;
        use std::task::{Context, Poll};

        let (access, account) = self.usable().await?;
        let url = format!("{}/responses", self.base_url);
        let body = build_body(&req);

        let mut request = self
            .http
            .post(&url)
            .bearer_auth(&access)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            // Named honestly. The client id is Codex's; this is not, and the
            // header is what the endpoint logs as the caller.
            .header("originator", super::oauth::codex::ORIGINATOR)
            .header("openai-beta", "responses=experimental");
        if let Some(account) = account {
            // Without it the token is refused: it authenticates a person, and
            // the endpoint still needs to know which account is being billed.
            request = request.header("chatgpt-account-id", account);
        }

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            r = request.json(&body).send() => r.map_err(|e| transport_error(e, &url))?,
        };

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body = response.text().await.unwrap_or_default();
            return Err(crate::anthropic::classify(
                status,
                retry_after.as_deref(),
                &body,
            ));
        }

        struct Body {
            inner: Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
            decoder: ResponsesStream,
            pending: std::collections::VecDeque<StreamEvent>,
            cancel: CancellationToken,
            finished: bool,
        }

        impl Stream for Body {
            type Item = Result<StreamEvent, ProviderError>;

            fn poll_next(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                loop {
                    if let Some(event) = self.pending.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    }
                    if self.finished {
                        return Poll::Ready(None);
                    }
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
                            return Poll::Ready(Some(Err(ProviderError::Transport(
                                Redacted::new(e.to_string()),
                            ))));
                        }
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

        Ok(Box::pin(Body {
            inner: Box::pin(response.bytes_stream()),
            decoder: ResponsesStream::default(),
            pending: std::collections::VecDeque::new(),
            cancel,
            finished: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_core::auth::{Discard, Secret};
    use std::sync::Mutex;

    fn tokens(expires_at_ms: u64) -> OAuthTokens {
        OAuthTokens {
            access: Secret::new("at"),
            refresh: Secret::new("rt"),
            expires_at_ms,
            account_id: Some("acct_1".into()),
        }
    }

    #[tokio::test]
    async fn a_live_token_is_used_without_a_round_trip() {
        // Far future, so any attempt to refresh would try to reach the network
        // and fail the test rather than pass it quietly.
        let provider = CodexProvider::new(tokens(u64::MAX), Arc::new(Discard), None).unwrap();
        let (access, account) = provider.usable().await.expect("the held token");
        assert_eq!(access, "at");
        assert_eq!(account.as_deref(), Some("acct_1"));
    }

    /// The margin is the point: a token with seconds left must be renewed
    /// before a turn that streams for longer than that.
    #[test]
    fn a_token_inside_the_margin_counts_as_expired() {
        let now = super::super::oauth::codex::now_ms();
        assert!(tokens(now + 1_000).expired(now, SKEW_MS));
        assert!(!tokens(now + SKEW_MS * 2).expired(now, SKEW_MS));
    }

    #[derive(Default)]
    struct Recording(Mutex<Vec<String>>);

    impl TokenSink for Recording {
        fn store(&self, provider: &str, _tokens: &OAuthTokens) {
            self.0.lock().unwrap().push(provider.to_owned());
        }
    }

    /// A refresh nobody persists is one every process repeats, and eventually
    /// one that spends a rotated refresh token.
    #[test]
    fn the_sink_is_told_which_provider_was_renewed() {
        let sink = Recording::default();
        sink.store(PROVIDER, &tokens(0));
        assert_eq!(sink.0.lock().unwrap().as_slice(), ["openai-codex"]);
    }

    #[test]
    fn the_base_url_can_be_overridden_but_defaults_to_the_subscription_endpoint() {
        let provider = CodexProvider::new(tokens(u64::MAX), Arc::new(Discard), None).unwrap();
        assert_eq!(provider.base_url, CODEX_BASE);

        let gateway = CodexProvider::new(
            tokens(u64::MAX),
            Arc::new(Discard),
            Some("https://x/".into()),
        )
        .unwrap();
        assert_eq!(gateway.base_url, "https://x/");
    }
}
