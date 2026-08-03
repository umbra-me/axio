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

/// Clean up a `Cookie:` header the way it arrives from a browser's network panel.
///
/// Every route someone actually copies one by adds something. "Copy value" gives the bare
/// pairs; copying the row gives `Cookie: a=1; b=2`; the raw-headers view gives lower-case
/// `cookie: a=1`; and a wrapped panel gives it back with newlines in the middle. Sent
/// verbatim, the second of those produces `Cookie: Cookie: a=1`, which a server reads as
/// one nameless cookie and none of the real ones — so a perfectly good session is refused.
///
/// Not defensive coding for its own sake: pasting a header is the entire credential flow
/// for three providers here, and the paste is the step that fails.
pub fn clean_cookie(raw: &str) -> Option<String> {
    let mut value = raw.trim();
    // Only a leading one is stripped: a cookie *value* may contain the word legitimately.
    for prefix in ["Cookie:", "cookie:", "COOKIE:"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.trim();
            break;
        }
    }
    // A wrapped paste arrives with newlines inside it; a cookie header is one line.
    let joined = value
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    let joined = joined.trim().trim_end_matches(';').trim().to_string();
    // No `=` means no cookie in there, whatever else the text is.
    (!joined.is_empty() && joined.contains('=')).then_some(joined)
}

/// Where a 3xx was pointing, for an error message that says something.
///
/// `Policy::none` means a redirect arrives here as a status rather than being followed, and
/// for these dashboard endpoints the destination *is* the diagnosis: `/signin` means the
/// session died, anything else means the request was wrong. Reported without its query
/// string, which on a sign-in redirect carries the original URL and sometimes a token.
pub fn redirect_target(response: &reqwest::Response) -> Option<String> {
    let raw = response
        .headers()
        .get(reqwest::header::LOCATION)?
        .to_str()
        .ok()?;
    Some(raw.split('?').next().unwrap_or(raw).to_string())
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;

    /// Where the probe reads its credentials from, for `axio-quota diagnose` output.
    fn credential_hint(&self, env: &Env) -> String;

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this function exists for: the network panel's "copy" gives you the
    /// header *including its name*, and sending that produces `Cookie: Cookie: a=1`.
    #[test]
    fn a_pasted_header_keeps_only_its_value() {
        assert_eq!(
            clean_cookie("Cookie: a=1; b=2").as_deref(),
            Some("a=1; b=2")
        );
        assert_eq!(clean_cookie("cookie: a=1").as_deref(), Some("a=1"));
        assert_eq!(clean_cookie("  a=1; b=2  ").as_deref(), Some("a=1; b=2"));
    }

    /// A wrapped panel gives the header back across several lines; a Cookie header is one.
    #[test]
    fn a_wrapped_paste_is_rejoined_into_one_line() {
        let pasted = "Cookie: session=abc;
  other=def;
  third=ghi";
        assert_eq!(
            clean_cookie(pasted).as_deref(),
            Some("session=abc; other=def; third=ghi")
        );
    }

    /// Only the leading name is stripped. A cookie value may say "cookie:" itself, and
    /// eating that would corrupt a working session.
    #[test]
    fn the_word_is_only_stripped_from_the_front() {
        assert_eq!(
            clean_cookie("consent=cookie:accepted").as_deref(),
            Some("consent=cookie:accepted")
        );
    }

    /// Nothing that could not be a cookie gets sent as one — an empty box, or a stray
    /// paste of something else, should read as "not configured" rather than as a refusal.
    #[test]
    fn text_that_holds_no_cookie_is_none() {
        assert_eq!(clean_cookie(""), None);
        assert_eq!(clean_cookie("   "), None);
        assert_eq!(clean_cookie("Cookie:"), None);
        assert_eq!(clean_cookie("no pairs here"), None);
    }

    #[test]
    fn a_trailing_semicolon_is_dropped() {
        assert_eq!(clean_cookie("a=1; b=2;").as_deref(), Some("a=1; b=2"));
    }
}
