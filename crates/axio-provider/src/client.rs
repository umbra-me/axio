//! The HTTP client, and the one place `rustls` is configured.

use std::sync::{Arc, Once};

use axio_core::provider::{
    BoxStream, ModelInfo, ModelRequest, Provider, ProviderError, StreamEvent,
};
use axio_core::redact::{Redacted, register_secret};
use tokio_util::sync::CancellationToken;

use crate::anthropic::{self, API_URL, API_VERSION};

static INSTALL_CRYPTO: Once = Once::new();

/// Install the `ring` crypto provider exactly once.
///
/// reqwest 0.13 defaults rustls to `aws-lc-rs`, whose `-sys` crate needs CMake
/// and NASM on Windows x86-64 — a `cargo install` failure on the platform we
/// can least afford one. We take `rustls-no-provider` and install `ring` here
/// instead. `--features aws-lc` is the escape hatch if a corporate proxy needs
/// a curve `ring` does not carry.
pub fn install_crypto_provider() {
    INSTALL_CRYPTO.call_once(|| {
        // An error means a provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub struct AnthropicProvider {
    http: reqwest::Client,
    api_key: Arc<str>,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Result<Self, ProviderError> {
        install_crypto_provider();
        let api_key: String = api_key.into();
        // Registered so it is scrubbed by value anywhere it might be echoed.
        register_secret(api_key.clone());

        let http = reqwest::Client::builder()
            .user_agent(concat!("axio/", env!("CARGO_PKG_VERSION")))
            // reqwest follows up to 10 redirects by default, re-sending the
            // `x-api-key` header and the full request body to whatever host the
            // hop names. An API client carrying a bearer-equivalent credential
            // has no legitimate reason to follow one.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ProviderError::Transport(Redacted::new(e.to_string())))?;

        Ok(Self {
            http,
            api_key: api_key.into(),
            base_url: API_URL.to_owned(),
        })
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn model_info(&self, _model: &str) -> ModelInfo {
        // One hardcoded model, so the price table cannot drift far. `axio
        // doctor` prints what it assumed.
        ModelInfo {
            context_window: 1_000_000,
            max_output_tokens: 128_000,
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
        use futures_core::Stream;
        use std::pin::Pin;
        use std::task::{Context, Poll};

        let body = anthropic::build_body(&req);

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            r = self.http
                .post(&self.base_url)
                .header("x-api-key", &*self.api_key)
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send() => r.map_err(|e| ProviderError::Transport(Redacted::new(e.to_string())))?,
        };

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body = response.text().await.unwrap_or_default();
            return Err(anthropic::classify(status, retry_after.as_deref(), &body));
        }

        struct Body {
            inner: Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
            decoder: anthropic::AnthropicStream,
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
                    if let Some(ev) = self.pending.pop_front() {
                        return Poll::Ready(Some(Ok(ev)));
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
            decoder: anthropic::AnthropicStream::new(),
            pending: std::collections::VecDeque::new(),
            cancel,
            finished: false,
        }))
    }
}

/// Read the credential, in precedence order.
///
/// On unix the file is expected to be `0600`; on Windows no file protection is
/// claimed, and the caller warns once and recommends the environment variable.
pub fn load_api_key() -> Result<String, ProviderError> {
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY")
        && !key.trim().is_empty()
    {
        return Ok(key);
    }
    Err(ProviderError::Auth(Redacted::new(
        "no credential found. Set ANTHROPIC_API_KEY, or run `axio auth login`.",
    )))
}
