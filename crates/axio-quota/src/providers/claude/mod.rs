//! Claude usage.
//!
//! Derived from upstream CodexBar's `ClaudeOAuthUsageFetcher` + `ClaudeOAuthCredentialModels`.
//!
//! Upstream reads the token from the macOS Keychain and only falls back to the file. On
//! Windows the file *is* the store: Claude Code writes `.credentials.json` under
//! `~/.claude`, so the fallback path becomes the primary one. That also means the token
//! sits in a plaintext file protected only by NTFS ACLs — see `docs/PORTING.md`.

mod credentials;
mod usage;

pub use credentials::{ClaudeCredentials, credentials_file, load_credentials, parse_credentials};
pub use usage::parse_usage;

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::error::ProbeError;
use crate::model::{ProviderId, UsageSnapshot};
use crate::paths::Env;
use crate::provider::{FetchContext, Provider};

const PROVIDER: &str = "Claude";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA_HEADER: &str = "oauth-2025-04-20";

pub struct ClaudeProvider;

#[async_trait]
impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn credential_hint(&self, env: &Env) -> String {
        credentials_file(env).display().to_string()
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let credentials = load_credentials(&ctx.env)?;
        if credentials.is_expired(OffsetDateTime::now_utc()) {
            // Refreshing would mean writing back a file Claude Code owns. Until that is
            // implemented, say so plainly rather than sending a token we know is dead.
            return Err(ProbeError::Unauthorized(
                "Claude token has expired. Run `claude` to refresh it.".to_string(),
            ));
        }

        let response = ctx
            .http
            .get(USAGE_URL)
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("Accept", "application/json")
            .header("anthropic-beta", BETA_HEADER)
            // No User-Agent override. Upstream CodexBar sends a Claude Code-shaped header
            // here, on the belief that the endpoint requires one. Tested against a live
            // account on 2026-08-02: it answers identically to `axio-quota/<version>`, so
            // there is no reason to present ourselves as another client.
            .send()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        let body = response
            .text()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        tracing::debug!(provider = PROVIDER, %status, %body, "usage response");

        match status.as_u16() {
            200..=299 => {
                let mut snapshot = parse_usage(&body)?;
                snapshot.plan = credentials.subscription_type;
                Ok(snapshot)
            }
            401 => Err(ProbeError::Unauthorized(
                "Claude request unauthorized. Run `claude` to re-authenticate.".to_string(),
            )),
            429 => Err(ProbeError::RateLimited { retry_after }),
            other => Err(ProbeError::Http {
                provider: PROVIDER,
                status: other,
                body: body.chars().take(400).collect(),
            }),
        }
    }
}
