//! Signing in, rather than pasting a key.
//!
//! Lives in this crate because it is HTTP: an authorize URL, a token endpoint,
//! and a listener for the one redirect that comes back. What it produces is an
//! `OAuthTokens` from `axio-core`, so nothing above here knows a flow happened.
//!
//! Opening the browser is deliberately *not* here. Spawning a process belongs
//! to `axio-tools`, which owns every subprocess in the workspace; this hands
//! back a URL and lets the caller decide how it gets opened.

mod callback;
pub mod codex;
mod pkce;

use axio_core::provider::ProviderError;
use axio_core::redact::Redacted;

/// An HTTP client for a sign-in.
///
/// Built the same way the providers build theirs — `ring` installed, redirects
/// refused — rather than `Client::new()`. A token exchange carries a credential
/// and follows the same rule as an API request: a redirect is a hop that would
/// be handed the credential, and no authorization endpoint needs one.
pub fn http_client() -> Result<reqwest::Client, ProviderError> {
    crate::client::install_crypto_provider();
    reqwest::Client::builder()
        .user_agent(concat!("axio/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| ProviderError::Transport(Redacted::new(e.to_string())))
}

/// Whether a provider is signed in to rather than given a key.
///
/// Asked in one place so the surface, the store and the flow cannot disagree
/// about which providers have a browser step — the failure being a provider
/// that asks for a paste and then has nowhere to send it.
pub fn is_oauth(provider: &str) -> bool {
    provider == "openai-codex"
}
