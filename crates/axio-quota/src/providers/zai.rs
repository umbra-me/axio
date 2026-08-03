//! z.ai coding-plan quota.
//!
//! An API token and one endpoint, so it follows the [`super::openrouter`] shape — but z.ai
//! reports real rate windows rather than a credit balance, and reports them in its own
//! vocabulary: a list of limits, each tagged with a type, a unit code and a count. A
//! `TIME_LIMIT` of unit 3 number 5 is a five-hour window; the same record with unit 6 is a
//! weekly one. That decoding is the whole of this file.

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::error::ProbeError;
use crate::json::{as_f64, as_i64, as_string, pick};
use crate::model::{ProviderId, RateWindow, UsageSnapshot};
use crate::paths::{Env, non_empty};
use crate::provider::{FetchContext, Provider};

const PROVIDER: &str = "z.ai";
const QUOTA_PATH: &str = "api/monitor/usage/quota/limit";
const DEFAULT_HOST: &str = "https://api.z.ai";
const API_KEY_ENV: &str = "Z_AI_API_KEY";
const HOST_ENV: &str = "Z_AI_API_HOST";

pub struct ZaiProvider;

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
        "No z.ai API key. Set one in config or export {API_KEY_ENV}."
    )))
}

/// The quota endpoint, honouring the region override.
///
/// Accounts in mainland China are served by BigModel on a different host. An override is
/// normalized to HTTPS and a bare host is accepted, but an explicit `http://` is refused
/// rather than upgraded: this request carries a bearer token, and quietly "fixing" a
/// plaintext URL would send it in the clear on the one occasion someone typed it wrong.
fn quota_url(env: &Env) -> Result<String, ProbeError> {
    let host = non_empty(env, HOST_ENV).unwrap_or(DEFAULT_HOST);
    let host = host.trim().trim_end_matches('/');
    if host.starts_with("http://") {
        return Err(ProbeError::NotConfigured(format!(
            "${HOST_ENV} is plaintext http://. z.ai requests carry a bearer token; use https://."
        )));
    }
    let base = if host.starts_with("https://") {
        host.to_string()
    } else {
        format!("https://{host}")
    };
    Ok(format!("{base}/{QUOTA_PATH}"))
}

/// z.ai's unit codes. Absent from the response's own vocabulary is any word for what a
/// window *is*, so the label is assembled from the unit and the count.
fn label_for(unit: i64, number: i64) -> String {
    match unit {
        1 if number == 1 => "Daily".to_string(),
        1 => format!("{number}d"),
        3 if number == 5 => "5h".to_string(),
        3 => format!("{number}h"),
        5 => format!("{number}m"),
        6 if number == 1 => "Weekly".to_string(),
        6 => format!("{number}w"),
        // An unrecognised unit is still a real limit with a real percentage. Reporting it
        // with a vague label beats dropping a window that might be the one about to run
        // out — z.ai has added units before.
        _ => format!("Limit ({number})"),
    }
}

fn window_minutes(unit: i64, number: i64) -> Option<u32> {
    let minutes = match unit {
        1 => number.checked_mul(24 * 60)?,
        3 => number.checked_mul(60)?,
        5 => number,
        6 => number.checked_mul(7 * 24 * 60)?,
        _ => return None,
    };
    u32::try_from(minutes).ok()
}

/// `{"code":200,"success":true,"data":{"limits":[…],"planName":"…"}}`.
pub fn parse_usage(raw: &str) -> Result<UsageSnapshot, ProbeError> {
    let root: serde_json::Value =
        serde_json::from_str(raw).map_err(|err| ProbeError::decode("z.ai quota response", err))?;

    // z.ai answers HTTP 200 with an error code in the body, so the transport status is not
    // the whole answer and the envelope has to be checked separately.
    let code = as_i64(pick(&root, &["code"])).unwrap_or(200);
    if code != 200 {
        let message = as_string(pick(&root, &["msg", "message"]))
            .unwrap_or_else(|| format!("z.ai returned code {code}"));
        return Err(match code {
            401 | 403 => ProbeError::Unauthorized(message),
            _ => ProbeError::decode("z.ai quota response", message),
        });
    }

    let data = root.get("data").unwrap_or(&root);
    let mut snapshot = UsageSnapshot::new(ProviderId::Zai);
    snapshot.plan = as_string(pick(
        data,
        &["planName", "plan", "plan_type", "packageName"],
    ));

    let limits = data
        .get("limits")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    for limit in limits {
        let Some(percentage) = as_f64(pick(limit, &["percentage"])) else {
            continue;
        };
        let unit = as_i64(pick(limit, &["unit"])).unwrap_or(0);
        let number = as_i64(pick(limit, &["number"])).unwrap_or(0);

        // Milliseconds since the epoch, which is the one unit in this response that is not
        // negotiable — a seconds value here would land in 1970 and read as "resetting".
        let resets_at = as_i64(pick(limit, &["nextResetTime"]))
            .filter(|millis| *millis > 0)
            .and_then(|millis| {
                OffsetDateTime::from_unix_timestamp_nanos(millis as i128 * 1_000_000).ok()
            });

        snapshot.windows.push(
            RateWindow::new(label_for(unit, number), percentage)
                .with_reset(resets_at)
                .with_window_minutes(window_minutes(unit, number)),
        );
    }

    if snapshot.windows.is_empty() {
        return Err(ProbeError::decode(
            "z.ai quota response",
            "no limits returned — a team token needs its organization and project ids",
        ));
    }
    Ok(snapshot)
}

#[async_trait]
impl Provider for ZaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Zai
    }

    fn credential_hint(&self, _env: &Env) -> String {
        format!("config apiKey, or ${API_KEY_ENV}")
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let key = api_key(ctx)?;
        let response = ctx
            .http
            .get(quota_url(&ctx.env)?)
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
                "z.ai rejected the API key.".to_string(),
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
    fn windows_are_named_from_the_unit_and_the_count() {
        let raw = r#"{"code":200,"success":true,"data":{"planName":"Coding Plan",
            "limits":[
              {"type":"TIME_LIMIT","unit":3,"number":5,"percentage":42,"nextResetTime":1785734400000},
              {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":8}
            ]}}"#;
        let snapshot = parse_usage(raw).expect("parses");

        assert_eq!(snapshot.plan.as_deref(), Some("Coding Plan"));
        assert_eq!(snapshot.windows[0].label, "5h");
        assert_eq!(snapshot.windows[0].used_percent, 42.0);
        assert_eq!(snapshot.windows[0].window_minutes, Some(300));
        assert!(snapshot.windows[0].resets_at.is_some());
        assert_eq!(snapshot.windows[1].label, "Weekly");
        assert_eq!(snapshot.windows[1].window_minutes, Some(10_080));
    }

    /// The reset stamp is milliseconds. Read as seconds it lands in 1970 and the window
    /// reports itself as permanently resetting.
    #[test]
    fn the_reset_stamp_is_milliseconds() {
        let raw = r#"{"code":200,"data":{"limits":[
            {"type":"TIME_LIMIT","unit":3,"number":5,"percentage":1,"nextResetTime":1785734400000}]}}"#;
        let at = parse_usage(raw).expect("parses").windows[0]
            .resets_at
            .expect("a reset");
        assert_eq!(at.year(), 2026, "seconds would give 1970");
    }

    /// A limit whose unit this build does not know is still a limit. Dropping it could
    /// hide the one window about to run out.
    #[test]
    fn an_unknown_unit_is_kept_with_a_vague_label() {
        let raw = r#"{"code":200,"data":{"limits":[
            {"type":"TIME_LIMIT","unit":99,"number":3,"percentage":77}]}}"#;
        let snapshot = parse_usage(raw).expect("parses");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].used_percent, 77.0);
        assert_eq!(snapshot.windows[0].window_minutes, None);
    }

    /// z.ai answers 200 with the error in the body, so the transport status is not the
    /// whole answer.
    #[test]
    fn an_error_code_in_a_200_body_is_still_an_error() {
        let raw = r#"{"code":401,"success":false,"msg":"invalid token"}"#;
        assert!(matches!(parse_usage(raw), Err(ProbeError::Unauthorized(_))));
    }

    /// Empty limits mean a team token missing its selectors, which is a configuration
    /// problem and must not read as "nothing used".
    #[test]
    fn no_limits_is_an_error_rather_than_an_empty_snapshot() {
        assert!(parse_usage(r#"{"code":200,"data":{"limits":[]}}"#).is_err());
    }

    /// The bearer token must never leave over plaintext, even if someone sets the override
    /// wrong. Upgrading it silently would hide the mistake exactly once, expensively.
    #[test]
    fn a_plaintext_host_override_is_refused_not_upgraded() {
        let env = |value: &str| {
            [(HOST_ENV.to_string(), value.to_string())]
                .into_iter()
                .collect::<Env>()
        };
        assert!(quota_url(&env("http://evil.example")).is_err());
        assert_eq!(
            quota_url(&env("open.bigmodel.cn")).unwrap(),
            format!("https://open.bigmodel.cn/{QUOTA_PATH}")
        );
        assert_eq!(
            quota_url(&Env::new()).unwrap(),
            format!("{DEFAULT_HOST}/{QUOTA_PATH}")
        );
    }
}
