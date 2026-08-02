//! Codex / ChatGPT usage.
//!
//! Derived from upstream CodexBar's `CodexOAuthUsageFetcher` + `CodexOAuthCredentialsStore`
//! rather than its `CodexStatusProbe`. Upstream has two paths to the same numbers: drive
//! the `codex` TUI in a pseudo-terminal and screen-scrape `/status`, or read the OAuth
//! token from `auth.json` and call the usage API. The PTY path would need ConPTY on
//! Windows and breaks whenever the TUI's layout changes; the API path is plain HTTP.

mod credentials;
mod usage;

pub use credentials::{CodexCredentials, auth_file, load_credentials, parse_credentials};
pub use usage::{parse_usage, usage_url};

use async_trait::async_trait;

use crate::error::ProbeError;
use crate::model::{ProviderId, UsageSnapshot};
use crate::paths::Env;
use crate::provider::{FetchContext, Provider};

const PROVIDER: &str = "Codex";

pub struct CodexProvider;

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn credential_hint(&self, env: &Env) -> String {
        auth_file(env).display().to_string()
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let credentials = load_credentials(&ctx.env)?;
        let url = usage_url(&ctx.env);

        let mut request = ctx
            .http
            .get(&url)
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("Accept", "application/json");
        if let Some(account_id) = &credentials.account_id {
            request = request.header("ChatGPT-Account-Id", account_id);
        }

        let response = request
            .send()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        // The bodies of usage endpoints carry no credentials, and being able to see the
        // payload a provider actually sent is the difference between a five-minute mapping
        // fix and an afternoon of guessing.
        tracing::debug!(provider = PROVIDER, %status, %body, "usage response");

        match status.as_u16() {
            200..=299 => parse_usage(&body),
            401 | 403 => Err(ProbeError::Unauthorized(
                "Codex token expired or invalid. Run `codex` to re-authenticate.".to_string(),
            )),
            429 => Err(ProbeError::RateLimited { retry_after: None }),
            other => Err(ProbeError::Http {
                provider: PROVIDER,
                status: other,
                body: body.chars().take(400).collect(),
            }),
        }
    }
}
