//! xAI developer-platform prepaid balance.
//!
//! Deliberately not the same thing as the Grok subscription that `axio cost` reads from
//! the Grok CLI's logs. This is the developer platform's prepaid billing ledger, reached
//! with a Management API key rather than an inference key, and the two share no credential
//! and no balance. Naming them apart is worth the confusion it prevents.

use async_trait::async_trait;

use crate::error::ProbeError;
use crate::json::{as_f64, as_string, pick};
use crate::model::{Credits, ProviderId, UsageSnapshot};
use crate::paths::{Env, non_empty};
use crate::provider::{FetchContext, Provider};

const PROVIDER: &str = "xAI";
const BASE: &str = "https://management-api.x.ai/v1/billing/teams";
const API_KEY_ENV: &str = "XAI_MANAGEMENT_API_KEY";
const TEAM_ENV: &str = "XAI_TEAM_ID";

pub struct XaiProvider;

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
        "No xAI Management API key. An inference key will not work here — create one under \
         Settings > Management Keys in the xAI console, or export {API_KEY_ENV}."
    )))
}

/// The team id, which is part of the URL rather than the credential.
fn team_id(ctx: &FetchContext) -> Result<String, ProbeError> {
    if let Some(id) = ctx
        .config
        .workspace_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return Ok(id.to_string());
    }
    if let Some(id) = non_empty(&ctx.env, TEAM_ENV) {
        return Ok(id.to_string());
    }
    Err(ProbeError::NotConfigured(format!(
        "No xAI team id. It is in the console URL; set workspaceID in config or export {TEAM_ENV}."
    )))
}

/// `{"total": "-1000"}` — a $10 balance.
///
/// The ledger is inverted and denominated in string cents, so a top-up is negative and the
/// remaining balance is the negated value over a hundred. A response with no parseable
/// total is an error and never a zero balance: "$0.00 left" and "we could not tell" are
/// different enough that showing the first for the second would be a lie the tray tells
/// silently.
pub fn parse_balance(raw: &str) -> Result<UsageSnapshot, ProbeError> {
    let root: serde_json::Value =
        serde_json::from_str(raw).map_err(|err| ProbeError::decode("xAI balance response", err))?;
    let data = root.get("data").unwrap_or(&root);

    let cents = as_f64(pick(
        data,
        &["total", "balance", "prepaid_balance", "prepaidBalance"],
    ));
    let Some(cents) = cents else {
        return Err(ProbeError::decode(
            "xAI balance response",
            "no parseable total — refusing to report this as a zero balance",
        ));
    };

    let dollars = -cents / 100.0;
    let mut snapshot = UsageSnapshot::new(ProviderId::Xai);
    snapshot.account_label = as_string(pick(data, &["team_name", "teamName"]));
    snapshot.credits = Some(Credits {
        balance: Some(dollars),
        unlimited: false,
        has_credits: dollars > 0.0,
    });
    // No window: a prepaid ledger has no ceiling to be a percentage of. The Providers view
    // shows the balance on its own rather than inventing a denominator to draw a rail with.
    Ok(snapshot)
}

#[async_trait]
impl Provider for XaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Xai
    }

    fn credential_hint(&self, _env: &Env) -> String {
        format!("config apiKey + workspaceID, or ${API_KEY_ENV} + ${TEAM_ENV}")
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let key = api_key(ctx)?;
        let team = team_id(ctx)?;
        let response = ctx
            .http
            .get(format!("{BASE}/{team}/prepaid/balance"))
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
            200..=299 => parse_balance(&body),
            401 | 403 => Err(ProbeError::Unauthorized(
                "xAI rejected the key. The Management API does not accept inference keys."
                    .to_string(),
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

    /// The ledger is inverted and in cents. Read straight through, a $10 balance reports
    /// as minus a thousand dollars.
    #[test]
    fn the_inverted_cent_ledger_becomes_dollars() {
        let snapshot = parse_balance(r#"{"total":"-1000"}"#).expect("parses");
        assert_eq!(snapshot.credits.as_ref().unwrap().balance, Some(10.0));
        assert!(snapshot.credits.unwrap().has_credits);
    }

    /// A spent-out team posts a positive ledger, which is a zero-or-worse balance.
    #[test]
    fn a_positive_ledger_is_an_exhausted_balance() {
        let credits = parse_balance(r#"{"total":"250"}"#)
            .unwrap()
            .credits
            .unwrap();
        assert_eq!(credits.balance, Some(-2.5));
        assert!(!credits.has_credits);
    }

    /// "We could not tell" must never render as "$0.00 left".
    #[test]
    fn an_unparseable_total_is_an_error_not_a_zero() {
        assert!(parse_balance(r#"{}"#).is_err());
        assert!(parse_balance(r#"{"total":null}"#).is_err());
        assert!(parse_balance(r#"{"total":"not a number"}"#).is_err());
    }

    /// A prepaid ledger has no ceiling, so there is no honest percentage to draw.
    #[test]
    fn a_balance_produces_no_window() {
        assert!(
            parse_balance(r#"{"total":"-1000"}"#)
                .unwrap()
                .windows
                .is_empty()
        );
    }
}
