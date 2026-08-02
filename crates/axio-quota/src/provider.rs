use std::time::Duration;

use async_trait::async_trait;

use crate::config::ProviderConfig;
use crate::error::ProbeError;
use crate::model::{ProviderId, UsageSnapshot};
use crate::paths::{Env, current_env};

/// Everything a probe is allowed to touch from the outside world.
///
/// Passing this in rather than letting probes reach for `std::env` and a global HTTP client
/// is what makes the whole crate testable: a test builds a context with `USERPROFILE`
/// pointing at a temp directory and the real credential-loading path runs unchanged.
#[derive(Clone)]
pub struct FetchContext {
    pub http: reqwest::Client,
    pub env: Env,
    pub config: ProviderConfig,
}

impl FetchContext {
    pub fn new(provider: ProviderId) -> Result<Self, ProbeError> {
        Ok(FetchContext {
            http: default_http_client()?,
            env: current_env(),
            config: ProviderConfig::new(provider),
        })
    }

    pub fn with_env(mut self, env: Env) -> Self {
        self.env = env;
        self
    }

    pub fn with_config(mut self, config: ProviderConfig) -> Self {
        self.config = config;
        self
    }
}

/// Installs `ring` as rustls' crypto provider.
///
/// The workspace takes rustls with `rustls-no-provider` so the default aws-lc-rs backend —
/// whose -sys crate needs CMake and NASM on Windows x86-64 — never enters the build. That
/// leaves no provider registered, so one must be installed before the first TLS handshake.
/// Idempotent: a second call returns `Err` and is ignored.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// A shared client so connections are pooled across refreshes.
///
/// The timeout is deliberately short: this runs on a refresh loop behind a tray icon, and a
/// probe that hangs for a minute is worse than one that fails and retries on the next tick.
pub fn default_http_client() -> Result<reqwest::Client, ProbeError> {
    install_crypto_provider();
    reqwest::Client::builder()
        .user_agent(concat!("axio-quota/", env!("CARGO_PKG_VERSION")))
        // Every probe here carries a bearer token. reqwest follows up to 10 redirects by
        // default, re-sending the Authorization header to whatever host the hop names —
        // and no usage endpoint has a legitimate reason to redirect one.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| ProbeError::Network {
            provider: "http",
            detail: err.to_string(),
        })
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;

    /// Where the probe reads its credentials from, for `axio-quota diagnose` output.
    fn credential_hint(&self, env: &Env) -> String;

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError>;
}
