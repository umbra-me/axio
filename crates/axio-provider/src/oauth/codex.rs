//! Signing in to ChatGPT, for the endpoint Codex talks to.
//!
//! Nothing is typed. The flow opens a browser, catches the redirect on the
//! loopback, exchanges the code and stores a token pair — pasting a key is the
//! thing this exists to avoid.
//!
//! **The client id below is Codex's own.** Presenting it is what makes the
//! authorization server accept a ChatGPT subscription for API use, and it is
//! outside what a third-party client is authorised to do. It is vendored here
//! deliberately and with that understood; it is a public identifier rather than
//! a secret, and the account being signed in to is the user's own.
//!
//! The redirect port is fixed rather than chosen. The authorization server only
//! accepts redirect URIs registered against the client, so a free port picked at
//! runtime is a `redirect_uri_mismatch` — the port being busy is a real failure
//! with a real message, not something to route around.

use std::time::Duration;

use axio_core::auth::{OAuthTokens, Secret};
use axio_core::provider::ProviderError;
use axio_core::redact::{Redacted, register_secret};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::pkce::{Pkce, random_urlsafe};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const AUTH_BASE: &str = "https://auth.openai.com";
pub const REDIRECT_PORT: u16 = 1455;
pub const SCOPE: &str = "openid profile email offline_access";
/// Who is asking. The client id is Codex's, but this is not — the parameter
/// names the program running the flow, and claiming to be the official client
/// as well as borrowing its id would be a second untruth for nothing.
pub const ORIGINATOR: &str = "axio";
/// Where the account id hides in the access token.
const CLAIM: &str = "https://api.openai.com/auth";

/// How long to wait for someone to finish in the browser before giving up.
const WAIT: Duration = Duration::from_secs(5 * 60);

pub fn redirect_uri() -> String {
    format!("http://localhost:{REDIRECT_PORT}/auth/callback")
}

/// Everything one attempt needs to remember between opening a browser and
/// redeeming what comes back.
pub struct Started {
    pub url: String,
    pkce: Pkce,
    state: String,
}

/// Build the authorize URL for a fresh attempt.
pub fn start() -> Result<Started, ProviderError> {
    let pkce = Pkce::generate()?;
    let state = random_urlsafe(16)?;
    let url = format!(
        "{AUTH_BASE}/oauth/authorize\
         ?response_type=code\
         &client_id={CLIENT_ID}\
         &redirect_uri={redirect}\
         &scope={scope}\
         &code_challenge={challenge}\
         &code_challenge_method=S256\
         &state={state}\
         &id_token_add_organizations=true\
         &codex_cli_simplified_flow=true\
         &originator={ORIGINATOR}",
        redirect = encode(&redirect_uri()),
        scope = encode(SCOPE),
        challenge = pkce.challenge,
    );
    Ok(Started { url, pkce, state })
}

/// Percent-encode the characters that appear in these values.
///
/// A general encoder is not needed and would be a dependency: the only values
/// spliced in here are a URL and a space-separated scope, so the set is `:`,
/// `/`, `?`, `&`, `=`, `+`, space, and anything non-alphanumeric that is not
/// already url-safe.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

impl Started {
    /// Wait for the browser to come back, then redeem what it brought.
    pub async fn finish(
        self,
        http: &reqwest::Client,
        cancel: CancellationToken,
    ) -> Result<OAuthTokens, ProviderError> {
        let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
            .await
            .map_err(|e| {
                ProviderError::Configuration(format!(
                    "cannot listen on 127.0.0.1:{REDIRECT_PORT} for the sign-in redirect ({e}). \
                     The port is registered with the provider and cannot be changed; something \
                     else is using it."
                ))
            })?;

        let code = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            _ = tokio::time::sleep(WAIT) => {
                return Err(ProviderError::Configuration(
                    "the sign-in was not completed in time".to_owned(),
                ));
            }
            caught = super::callback::catch(&listener, &self.state) => caught?,
        };

        exchange(http, &code, &self.pkce.verifier).await
    }
}

/// Redeem an authorization code.
async fn exchange(
    http: &reqwest::Client,
    code: &str,
    verifier: &str,
) -> Result<OAuthTokens, ProviderError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT_ID),
        ("code", code),
        ("redirect_uri", &redirect_uri()),
        ("code_verifier", verifier),
    ];
    post_token(http, &form).await
}

/// Trade a refresh token for a fresh pair.
pub async fn refresh(
    http: &reqwest::Client,
    refresh_token: &str,
) -> Result<OAuthTokens, ProviderError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", CLIENT_ID),
        ("refresh_token", refresh_token),
        ("scope", SCOPE),
    ];
    post_token(http, &form).await
}

async fn post_token(
    http: &reqwest::Client,
    form: &[(&str, &str)],
) -> Result<OAuthTokens, ProviderError> {
    // Encoded here rather than with reqwest's `form`, which is behind a feature
    // this workspace does not enable. The encoder is already needed for the
    // authorize URL, and one encoder used by both is one set of rules.
    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let response = http
        .post(format!("{AUTH_BASE}/oauth/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| crate::client::transport_error(e, AUTH_BASE))?;

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(crate::anthropic::classify(status, None, &body));
    }
    tokens_from(&body, now_ms())
}

/// Read a token response into the pair that gets stored.
pub(super) fn tokens_from(body: &str, now_ms: u64) -> Result<OAuthTokens, ProviderError> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::Transport(Redacted::new(format!("token response: {e}"))))?;

    let field = |name: &str| {
        parsed
            .get(name)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned)
    };

    let Some(access) = field("access_token") else {
        return Err(ProviderError::Transport(Redacted::new(
            "the token response carried no access_token".to_owned(),
        )));
    };
    // Without this the pair cannot be renewed, and the sign-in is good until it
    // silently is not. Better to fail now, while someone is watching a browser.
    let Some(refresh) = field("refresh_token") else {
        return Err(ProviderError::Transport(Redacted::new(
            "the token response carried no refresh_token, so it could never be renewed".to_owned(),
        )));
    };

    register_secret(access.clone());
    register_secret(refresh.clone());

    let lifetime_ms = parsed
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600)
        .saturating_mul(1000);

    Ok(OAuthTokens {
        account_id: account_id(&access),
        access: Secret::new(access),
        refresh: Secret::new(refresh),
        expires_at_ms: now_ms.saturating_add(lifetime_ms),
    })
}

/// The account the token speaks for, out of its own payload.
///
/// The token is not verified here and must not be: verifying it is the
/// endpoint's job, and this only reads a claim to put in a header. A payload
/// that will not decode costs the header, not the sign-in.
pub(super) fn account_id(access: &str) -> Option<String> {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let payload = access.split('.').nth(1)?;
    let raw = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&raw).ok()?;
    claims
        .get(CLAIM)?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_owned)
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authorize_url_carries_what_the_exchange_will_be_checked_against() {
        let started = start().expect("randomness");
        assert!(
            started
                .url
                .starts_with(&format!("{AUTH_BASE}/oauth/authorize?"))
        );
        assert!(started.url.contains("code_challenge_method=S256"));
        // The regression: without these two the authorize request is refused
        // as `missing_required_parameter`, and the only place that says so is a
        // browser page the program never sees.
        assert!(started.url.contains("codex_cli_simplified_flow=true"));
        assert!(started.url.contains(&format!("originator={ORIGINATOR}")));
        assert!(started.url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(started.url.contains(&format!("state={}", started.state)));
        // The verifier is the secret half and must never be in the URL.
        assert!(
            !started.url.contains(&started.pkce.verifier),
            "the verifier leaked into the authorize URL"
        );
    }

    #[test]
    fn the_redirect_is_encoded_rather_than_spliced_raw() {
        let started = start().expect("randomness");
        assert!(
            started
                .url
                .contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455")
        );
        assert!(!started.url.contains("redirect_uri=http://"));
    }

    #[test]
    fn a_pair_is_read_out_of_a_token_response() {
        let body = r#"{"access_token":"at","refresh_token":"rt","expires_in":3600}"#;
        let tokens = tokens_from(body, 1_000).expect("a pair");
        assert_eq!(tokens.access.expose(), "at");
        assert_eq!(tokens.refresh.expose(), "rt");
        assert_eq!(tokens.expires_at_ms, 1_000 + 3_600_000);
    }

    /// A pair without a refresh token works until it does not, and then there
    /// is nothing to renew it with. Better to fail while a browser is open.
    #[test]
    fn a_response_that_cannot_be_renewed_is_refused() {
        let body = r#"{"access_token":"at","expires_in":3600}"#;
        assert!(tokens_from(body, 0).is_err());
        assert!(tokens_from(r#"{"refresh_token":"rt"}"#, 0).is_err());
        assert!(tokens_from("not json", 0).is_err());
    }

    #[test]
    fn the_account_id_is_read_from_the_token_payload() {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let claims = serde_json::json!({ CLAIM: { "chatgpt_account_id": "acct_42" } });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let jwt = format!("header.{payload}.signature");

        let body = format!(r#"{{"access_token":"{jwt}","refresh_token":"rt","expires_in":60}}"#);
        let tokens = tokens_from(&body, 0).expect("a pair");
        assert_eq!(tokens.account_id.as_deref(), Some("acct_42"));
    }

    /// The token is not verified here, so a payload that will not decode costs
    /// the header and not the sign-in.
    #[test]
    fn an_unreadable_payload_costs_the_header_not_the_signin() {
        let body = r#"{"access_token":"opaque-not-a-jwt","refresh_token":"rt"}"#;
        let tokens = tokens_from(body, 0).expect("a pair");
        assert_eq!(tokens.account_id, None);
    }
}
