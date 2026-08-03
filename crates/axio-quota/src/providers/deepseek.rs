//! DeepSeek prepaid balance.
//!
//! The API-key half only. DeepSeek's richer per-model spend lives behind private dashboard
//! endpoints that need a browser session token, and reading someone's browser storage is a
//! different privacy proposition from reading a key they typed into this app. The balance
//! is what a tray icon can act on anyway: it is the number that reaches zero.

use async_trait::async_trait;

use crate::error::ProbeError;
use crate::json::{as_bool, as_f64, as_string, pick};
use crate::model::{Credits, ProviderId, RateWindow, UsageSnapshot};
use crate::paths::{Env, non_empty};
use crate::provider::{FetchContext, Provider};

const PROVIDER: &str = "DeepSeek";
const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";
const API_KEY_ENV: &str = "DEEPSEEK_API_KEY";

pub struct DeepSeekProvider;

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
        "No DeepSeek API key. Set one in config or export {API_KEY_ENV}."
    )))
}

/// `{"is_available":true,"balance_infos":[{"currency":"USD","total_balance":"9.50",…}]}`.
///
/// Amounts arrive as strings, and there is one entry per currency. USD is preferred when
/// present and the first entry taken otherwise, rather than summing: adding CNY to USD
/// would produce a number that is not money in any currency.
pub fn parse_usage(raw: &str) -> Result<UsageSnapshot, ProbeError> {
    let root: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| ProbeError::decode("DeepSeek balance response", err))?;

    let infos = root
        .get("balance_infos")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let chosen = infos
        .iter()
        .find(|info| {
            as_string(pick(info, &["currency"]))
                .is_some_and(|currency| currency.eq_ignore_ascii_case("USD"))
        })
        .or_else(|| infos.first());

    let Some(info) = chosen else {
        return Err(ProbeError::decode(
            "DeepSeek balance response",
            "no balance_infos entry",
        ));
    };

    let currency = as_string(pick(info, &["currency"])).unwrap_or_else(|| "USD".to_string());
    let total = as_f64(pick(info, &["total_balance", "totalBalance"]));
    let granted = as_f64(pick(info, &["granted_balance", "grantedBalance"])).unwrap_or(0.0);
    let topped_up = as_f64(pick(info, &["topped_up_balance", "toppedUpBalance"])).unwrap_or(0.0);

    let Some(total) = total else {
        return Err(ProbeError::decode(
            "DeepSeek balance response",
            "missing total_balance",
        ));
    };

    let mut snapshot = UsageSnapshot::new(ProviderId::Deepseek);
    snapshot.plan = Some(currency);
    snapshot.account_label = as_bool(pick(&root, &["is_available"]))
        .filter(|available| !available)
        .map(|_| "Suspended".to_string());

    // A balance is what remains, not what was used, and there is no ceiling to divide by:
    // DeepSeek does not report the original top-up. Rather than invent a denominator, the
    // window is the share of the balance that is *granted* credit — the part that expires
    // and cannot be recovered by paying. Where nothing was granted, there is no window and
    // the credits figure stands alone.
    if granted > 0.0 && total > 0.0 {
        let used_percent = (1.0 - (granted / total)).clamp(0.0, 100.0) * 100.0;
        snapshot
            .windows
            .push(RateWindow::new("Granted credit spent", used_percent));
    }

    snapshot.credits = Some(Credits {
        balance: Some(total),
        unlimited: false,
        has_credits: total > 0.0,
    });
    let _ = topped_up;
    Ok(snapshot)
}

#[async_trait]
impl Provider for DeepSeekProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Deepseek
    }

    fn credential_hint(&self, _env: &Env) -> String {
        format!("config apiKey, or ${API_KEY_ENV}")
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let key = api_key(ctx)?;
        let response = ctx
            .http
            .get(BALANCE_URL)
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
                "DeepSeek rejected the API key.".to_string(),
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
    fn the_balance_is_read_from_the_usd_entry() {
        let raw = r#"{"is_available":true,"balance_infos":[
            {"currency":"CNY","total_balance":"70.00","granted_balance":"0.00","topped_up_balance":"70.00"},
            {"currency":"USD","total_balance":"9.50","granted_balance":"4.75","topped_up_balance":"4.75"}]}"#;
        let snapshot = parse_usage(raw).expect("parses");

        assert_eq!(snapshot.plan.as_deref(), Some("USD"));
        assert_eq!(snapshot.credits.as_ref().unwrap().balance, Some(9.5));
        assert_eq!(snapshot.windows[0].used_percent, 50.0);
    }

    /// Amounts are strings on the wire. Read as numbers they come back as None and the
    /// balance silently reports as missing.
    #[test]
    fn string_amounts_are_parsed_as_numbers() {
        let raw = r#"{"balance_infos":[{"currency":"USD","total_balance":"12.25"}]}"#;
        assert_eq!(
            parse_usage(raw).unwrap().credits.unwrap().balance,
            Some(12.25)
        );
    }

    /// Currencies are not summed. Adding CNY to USD produces a number that is not money.
    #[test]
    fn a_single_non_usd_entry_is_used_as_is() {
        let raw = r#"{"balance_infos":[{"currency":"CNY","total_balance":"70.00"}]}"#;
        let snapshot = parse_usage(raw).expect("parses");
        assert_eq!(snapshot.plan.as_deref(), Some("CNY"));
        assert_eq!(snapshot.credits.unwrap().balance, Some(70.0));
    }

    #[test]
    fn a_suspended_account_says_so() {
        let raw = r#"{"is_available":false,"balance_infos":[{"currency":"USD","total_balance":"0"}]}"#;
        let snapshot = parse_usage(raw).expect("parses");
        assert_eq!(snapshot.account_label.as_deref(), Some("Suspended"));
        assert!(!snapshot.credits.unwrap().has_credits);
    }

    #[test]
    fn an_empty_response_is_an_error_rather_than_a_zero_balance() {
        assert!(parse_usage(r#"{"balance_infos":[]}"#).is_err());
        assert!(parse_usage(r#"{"balance_infos":[{"currency":"USD"}]}"#).is_err());
    }
}
