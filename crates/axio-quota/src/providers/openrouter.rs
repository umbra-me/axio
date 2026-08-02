//! OpenRouter credits.
//!
//! The template for every API-key provider: no local credential discovery, no OAuth, just
//! a key from config (or the environment) and one JSON endpoint. Adding Groq, DeepSeek, or
//! Mistral should mean copying this file and changing the URL and the field mapping.

use async_trait::async_trait;

use crate::error::ProbeError;
use crate::json::{as_f64, pick};
use crate::model::{Credits, ProviderId, RateWindow, UsageSnapshot};
use crate::paths::{Env, non_empty};
use crate::provider::{FetchContext, Provider};

const PROVIDER: &str = "OpenRouter";
const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";
const API_KEY_ENV: &str = "OPENROUTER_API_KEY";

pub struct OpenRouterProvider;

/// Config first, environment second.
///
/// The config file is the setting the user manages in the app; the environment variable is
/// the escape hatch for scripts and CI. If the environment won, a stale shell variable
/// would silently override what the UI shows as configured.
fn api_key(ctx: &FetchContext) -> Result<String, ProbeError> {
    if let Some(key) = ctx
        .config
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        return Ok(key.to_string());
    }
    if let Some(key) = non_empty(&ctx.env, API_KEY_ENV) {
        return Ok(key.to_string());
    }
    Err(ProbeError::NotConfigured(format!(
        "No OpenRouter API key. Set one in config or export {API_KEY_ENV}."
    )))
}

/// `{"data": {"total_credits": 20.0, "total_usage": 4.5}}` — an amount purchased and an
/// amount spent, not a percentage. The percentage is ours to compute.
pub fn parse_usage(raw: &str) -> Result<UsageSnapshot, ProbeError> {
    let root: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| ProbeError::decode("OpenRouter credits response", err))?;
    let data = root.get("data").unwrap_or(&root);

    let total = as_f64(pick(data, &["total_credits", "totalCredits"]));
    let used = as_f64(pick(data, &["total_usage", "totalUsage"]));

    let (Some(total), Some(used)) = (total, used) else {
        return Err(ProbeError::decode(
            "OpenRouter credits response",
            "missing total_credits or total_usage",
        ));
    };

    let mut snapshot = UsageSnapshot::new(ProviderId::Openrouter);
    // A zero total means a pay-as-you-go account with nothing purchased: reporting 100%
    // used would light up the tray red for an account that is working fine.
    let used_percent = if total > 0.0 {
        (used / total * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    snapshot
        .windows
        .push(RateWindow::new("Credits", used_percent));
    snapshot.credits = Some(Credits {
        balance: Some((total - used).max(0.0)),
        unlimited: false,
        has_credits: total > used,
    });
    Ok(snapshot)
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Openrouter
    }

    fn credential_hint(&self, _env: &Env) -> String {
        format!("config apiKey, or ${API_KEY_ENV}")
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let key = api_key(ctx)?;
        let response = ctx
            .http
            .get(CREDITS_URL)
            .header("Authorization", format!("Bearer {key}"))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        match status.as_u16() {
            200..=299 => parse_usage(&body),
            401 | 403 => Err(ProbeError::Unauthorized(
                "OpenRouter rejected the API key.".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_percent_and_balance_from_totals() {
        let snapshot = parse_usage(r#"{ "data": { "total_credits": 20, "total_usage": 5 } }"#)
            .expect("parses");
        assert_eq!(snapshot.windows[0].used_percent, 25.0);
        assert_eq!(snapshot.credits.as_ref().unwrap().balance, Some(15.0));
        assert!(snapshot.credits.as_ref().unwrap().has_credits);
    }

    #[test]
    fn a_zero_total_reads_as_empty_not_exhausted() {
        let snapshot =
            parse_usage(r#"{ "data": { "total_credits": 0, "total_usage": 0 } }"#).expect("parses");
        assert_eq!(snapshot.windows[0].used_percent, 0.0);
    }

    #[test]
    fn overspend_is_clamped_to_full() {
        let snapshot = parse_usage(r#"{ "data": { "total_credits": 10, "total_usage": 12 } }"#)
            .expect("parses");
        assert_eq!(snapshot.windows[0].used_percent, 100.0);
        assert_eq!(snapshot.credits.as_ref().unwrap().balance, Some(0.0));
    }
}
