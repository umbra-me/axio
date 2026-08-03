//! Cursor plan usage.
//!
//! Cursor is web-backed: there is no API-key surface for usage, only the same endpoint the
//! dashboard calls with a session cookie. So this takes a pasted `Cookie:` header, which
//! the config already carries for exactly this case.
//!
//! Cursor also writes an access token into its own local state database, and CodexBar
//! derives a session cookie from it so nothing has to be pasted. That path is deliberately
//! not taken here yet: reading it means linking a native SQLite dependency into a crate
//! that currently has none, which is a decision to make on purpose rather than in passing.
//! [`session_cookie`] is the half of it that needs no dependency, and is tested, so
//! wiring the database read in later is a small change rather than a new design.

use async_trait::async_trait;

use crate::error::ProbeError;
use crate::json::{as_f64, as_i64, as_string, pick};
use crate::model::{Credits, ProviderId, RateWindow, UsageSnapshot};
use crate::paths::Env;
use crate::provider::{FetchContext, Provider, redirect_target};

const PROVIDER: &str = "Cursor";
const SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";

pub struct CursorProvider;

fn cookie(ctx: &FetchContext) -> Result<String, ProbeError> {
    ctx.config
        .cookie_header
        .as_deref()
        .and_then(|raw| super::cookie_header_for(ProviderId::Cursor, raw))
        .ok_or_else(|| {
            ProbeError::NotConfigured(
                "No Cursor session. Open cursor.com signed in, copy the Cookie header from any \
                 request in the Network tab, and paste it into Settings."
                    .to_string(),
            )
        })
}

/// Build Cursor's session cookie from a user id and an access token.
///
/// The two are joined by a URL-encoded `::`, which is the format Cursor's own web session
/// uses. Kept here, and tested, because it is the only non-obvious part of reading the
/// token out of Cursor's local state — the rest is a SQLite lookup.
pub fn session_cookie(user_id: &str, access_token: &str) -> String {
    format!("WorkosCursorSessionToken={user_id}%3A%3A{access_token}")
}

/// Percentages that are already percentages.
///
/// Cursor reports these in percentage units even when they are below one: `0.36` means
/// 0.36%, not 36%. Treating a fraction as a fraction here would turn a rounding-to-zero
/// reading into a third of the plan.
fn percent(value: Option<&serde_json::Value>) -> Option<f64> {
    as_f64(value).map(|raw| raw.clamp(0.0, 100.0))
}

/// `{"membershipType":"pro","billingCycleEnd":"…","individualUsage":{"plan":{…},"onDemand":{…}}}`
pub fn parse_usage(raw: &str) -> Result<UsageSnapshot, ProbeError> {
    let root: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| ProbeError::decode("Cursor usage-summary response", err))?;

    let mut snapshot = UsageSnapshot::new(ProviderId::Cursor);
    snapshot.plan = as_string(pick(&root, &["membershipType", "limitType"]));

    let resets_at = as_string(pick(&root, &["billingCycleEnd"])).and_then(|text| {
        time::OffsetDateTime::parse(&text, &time::format_description::well_known::Rfc3339).ok()
    });

    let individual = root.get("individualUsage").unwrap_or(&serde_json::Value::Null);
    let plan = individual.get("plan").unwrap_or(&serde_json::Value::Null);

    for (field, label) in [
        ("autoPercentUsed", "Auto"),
        ("apiPercentUsed", "Included"),
    ] {
        if let Some(used) = percent(pick(plan, &[field])) {
            snapshot
                .windows
                .push(RateWindow::new(label, used).with_reset(resets_at));
        }
    }

    // On-demand and the enterprise personal cap are both in cents, and both are a spend
    // against a ceiling rather than a percentage. Reported as a window only when there is
    // a ceiling to be a percentage of.
    for (key, label) in [("onDemand", "On demand"), ("overall", "Personal cap")] {
        let bucket = individual.get(key).unwrap_or(&serde_json::Value::Null);
        let used = as_i64(pick(bucket, &["used"]));
        let limit = as_i64(pick(bucket, &["limit"]));
        if let (Some(used), Some(limit)) = (used, limit)
            && limit > 0
        {
            snapshot.windows.push(
                RateWindow::new(label, (used as f64 / limit as f64 * 100.0).clamp(0.0, 100.0))
                    .with_reset(resets_at),
            );
            if key == "onDemand" {
                snapshot.credits = Some(Credits {
                    balance: Some((limit - used).max(0) as f64 / 100.0),
                    unlimited: false,
                    has_credits: limit > used,
                });
            }
        }
    }

    // An unlimited plan reports no percentage anywhere, which is a real state and not a
    // parse failure — but it must not render as an empty provider either.
    if snapshot.windows.is_empty() {
        if pick(&root, &["isUnlimited"]).and_then(serde_json::Value::as_bool) == Some(true) {
            snapshot.credits = Some(Credits {
                balance: None,
                unlimited: true,
                has_credits: true,
            });
            return Ok(snapshot);
        }
        return Err(ProbeError::decode(
            "Cursor usage-summary response",
            "no usage figures — the session may have expired",
        ));
    }
    Ok(snapshot)
}

#[async_trait]
impl Provider for CursorProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Cursor
    }

    fn credential_hint(&self, _env: &Env) -> String {
        "config cookieHeader — paste a Cookie: header from cursor.com".to_string()
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let response = ctx
            .http
            .get(SUMMARY_URL)
            .header("Cookie", cookie(ctx)?)
            .header("Accept", "application/json")
            // A dashboard endpoint that sees no Origin or Referer treats the call as
            // cross-site rather than as a session, which from outside is indistinguishable
            // from a rejected cookie.
            .header("Origin", "https://cursor.com")
            .header("Referer", "https://cursor.com/dashboard")
            .send()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        let status = response.status();
        let location = redirect_target(&response);
        let body = response
            .text()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        match status.as_u16() {
            200..=299 => parse_usage(&body),
            // A dashboard endpoint answers an expired session with a redirect to sign-in
            // as readily as with a 401, and `Policy::none` leaves the 3xx to be read here.
            300..=399 | 401 | 403 => Err(ProbeError::Unauthorized(format!(
                "Cursor answered {status}{}. The header needs the WorkosCursorSessionToken \
                 cookie — copy the whole `cookie:` line from a cursor.com request.",
                location
                    .map(|to| format!(", redirecting to {to}"))
                    .unwrap_or_default()
            ))),
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
    fn plan_percentages_become_windows() {
        let raw = r#"{"membershipType":"pro","billingCycleEnd":"2026-09-01T00:00:00Z",
            "individualUsage":{"plan":{"autoPercentUsed":42.5,"apiPercentUsed":8.0}}}"#;
        let snapshot = parse_usage(raw).expect("parses");

        assert_eq!(snapshot.plan.as_deref(), Some("pro"));
        assert_eq!(snapshot.windows[0].label, "Auto");
        assert_eq!(snapshot.windows[0].used_percent, 42.5);
        assert_eq!(snapshot.windows[1].label, "Included");
        assert!(snapshot.windows[0].resets_at.is_some());
    }

    /// Cursor's percentages are percentages even below one. Read as a fraction, a reading
    /// the dashboard rounds to 0% would render as a third of the plan gone.
    #[test]
    fn a_fractional_percentage_stays_a_percentage() {
        let raw = r#"{"individualUsage":{"plan":{"autoPercentUsed":0.36}}}"#;
        assert_eq!(parse_usage(raw).unwrap().windows[0].used_percent, 0.36);
    }

    /// On-demand arrives in cents against a cents ceiling.
    #[test]
    fn on_demand_cents_become_a_percentage_and_a_balance() {
        let raw = r#"{"individualUsage":{"onDemand":{"enabled":true,"used":7384,"limit":10000}}}"#;
        let snapshot = parse_usage(raw).expect("parses");
        assert_eq!(snapshot.windows[0].label, "On demand");
        assert!((snapshot.windows[0].used_percent - 73.84).abs() < 1e-9);
        assert_eq!(snapshot.credits.unwrap().balance, Some(26.16));
    }

    /// No ceiling means no percentage. Dividing by zero would report either NaN or a
    /// confident 100%, and both are worse than leaving the window out.
    #[test]
    fn a_bucket_with_no_ceiling_produces_no_window() {
        let raw = r#"{"isUnlimited":true,"individualUsage":{"onDemand":{"used":500,"limit":0}}}"#;
        let snapshot = parse_usage(raw).expect("parses");
        assert!(snapshot.windows.is_empty());
        assert!(snapshot.credits.unwrap().unlimited);
    }

    /// An empty answer is an expired session far more often than an idle account, so it
    /// must not render as 0% used.
    #[test]
    fn nothing_at_all_is_an_error_rather_than_zero_usage() {
        assert!(parse_usage(r#"{"membershipType":"pro"}"#).is_err());
    }

    /// The `::` between the two halves is URL-encoded; sent raw, Cursor reads the cookie
    /// as a user id with no token.
    #[test]
    fn the_session_cookie_encodes_its_separator() {
        let cookie = session_cookie("auth0|user_01ABC", "ey.token");
        assert_eq!(cookie, "WorkosCursorSessionToken=auth0|user_01ABC%3A%3Aey.token");
        assert!(!cookie.contains("::"));
    }
}
