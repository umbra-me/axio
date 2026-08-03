//! opencode subscription windows.
//!
//! The response is the awkward part. opencode's server functions answer with
//! `text/javascript` carrying a serialized object graph rather than JSON, so there is
//! nothing to hand to serde: the figures have to be lifted out of the text by name.
//!
//! That is done the same way [`super::ollama`] reads a rendered page — look near a known
//! label, inside a bounded window, and fail loudly when the shape is not there. The bound
//! is what stops `rollingUsage` picking up the weekly figure when the rolling one is
//! missing, which is the specific way this kind of extraction goes wrong quietly.

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};

use crate::error::ProbeError;
use crate::model::{ProviderId, RateWindow, UsageSnapshot};
use crate::paths::Env;
use crate::provider::{FetchContext, Provider};

const PROVIDER: &str = "opencode";
const SERVER_URL: &str = "https://opencode.ai/_server";

/// The server function that returns the subscription's usage windows. opencode addresses
/// its server functions by content hash rather than by name, so this is the name.
const SUBSCRIPTION_FN: &str = "7abeebee372f304e050aaaf92be863f4a86490e382f8c79db68fd94040d691b4";

/// How far past a label to look for its numbers. Wide enough for the fields between them,
/// narrow enough that a missing figure cannot borrow the next window's.
const NEAR: usize = 400;

pub struct OpenCodeProvider;

fn cookie(ctx: &FetchContext) -> Result<String, ProbeError> {
    ctx.config
        .cookie_header
        .as_deref()
        .map(str::trim)
        .filter(|header| !header.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            ProbeError::NotConfigured(
                "No opencode session. Open opencode.ai signed in, copy the Cookie header from \
                 any request in the Network tab, and paste it into Settings."
                    .to_string(),
            )
        })
}

/// A number written after `"field":` or `field:`, within `NEAR` bytes of `label`.
fn number_near(text: &str, label: &str, field: &str) -> Option<f64> {
    let at = text.find(label)?;
    let window = &text[at..text.len().min(at + NEAR)];
    let key = window.find(field)?;
    let rest = &window[key + field.len()..];
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit() && *c != '-')
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    digits.parse().ok()
}

/// Lift the usage windows out of the serialized response.
///
/// `now` is a parameter because the response gives a countdown rather than a timestamp:
/// resets arrive as seconds remaining, and turning that into a time needs a clock the test
/// can also hold.
pub fn parse_usage(text: &str, now: OffsetDateTime) -> Result<UsageSnapshot, ProbeError> {
    let mut snapshot = UsageSnapshot::new(ProviderId::Opencode);

    for (label, name, minutes) in [
        ("rollingUsage", "5h", Some(300u32)),
        ("weeklyUsage", "Weekly", Some(10_080)),
    ] {
        let Some(used) = number_near(text, label, "usagePercent") else {
            continue;
        };
        let resets_at = number_near(text, label, "resetInSec")
            .filter(|seconds| *seconds > 0.0)
            .map(|seconds| now + Duration::seconds(seconds as i64));
        snapshot.windows.push(
            RateWindow::new(name, used.clamp(0.0, 100.0))
                .with_reset(resets_at)
                .with_window_minutes(minutes),
        );
    }

    // The rolling window is the one opencode always reports; weekly is optional. Missing
    // both means the response was not the one expected, which is a parse failure and not
    // an idle account.
    if snapshot.windows.is_empty() {
        return Err(ProbeError::decode(
            "opencode subscription response",
            "no rolling usage found — the session may have expired",
        ));
    }
    Ok(snapshot)
}

#[async_trait]
impl Provider for OpenCodeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Opencode
    }

    fn credential_hint(&self, _env: &Env) -> String {
        "config cookieHeader, plus workspaceID to skip the workspace lookup".to_string()
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let workspace = ctx
            .config
            .workspace_id
            .as_deref()
            .map(str::trim)
            // A pasted URL is what someone has to hand; the id is the last path segment.
            .map(|raw| raw.rsplit('/').next().unwrap_or(raw))
            .filter(|id| !id.is_empty())
            .unwrap_or_default();

        let response = ctx
            .http
            .post(SERVER_URL)
            .header("Cookie", cookie(ctx)?)
            .header("Content-Type", "application/json")
            .header("Accept", "text/javascript, application/json")
            .header("X-Server-Fn", SUBSCRIPTION_FN)
            .body(format!("[{{\"workspaceID\":\"{workspace}\"}}]"))
            .send()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| ProbeError::network(PROVIDER, err))?;

        match status.as_u16() {
            200..=299 => parse_usage(&body, OffsetDateTime::now_utc()),
            300..=399 | 401 | 403 => Err(ProbeError::Unauthorized(
                "opencode rejected the session cookie. Sign in at opencode.ai and paste a fresh \
                 Cookie header."
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
    use time::format_description::well_known::Rfc3339;

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-08-03T12:00:00Z", &Rfc3339).expect("fixed stamp")
    }

    const BODY: &str = r#"0:{"subscription":{"plan":"pro",
        "rollingUsage":{"usagePercent":42.5,"resetInSec":3600},
        "weeklyUsage":{"usagePercent":8,"resetInSec":172800}}}"#;

    #[test]
    fn both_windows_are_lifted_with_their_countdowns() {
        let snapshot = parse_usage(BODY, now()).expect("parses");
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].label, "5h");
        assert_eq!(snapshot.windows[0].used_percent, 42.5);
        assert_eq!(
            snapshot.windows[0].resets_at,
            Some(now() + Duration::hours(1))
        );
        assert_eq!(snapshot.windows[1].label, "Weekly");
        assert_eq!(snapshot.windows[1].used_percent, 8.0);
    }

    /// Weekly is optional and the rolling window is not.
    #[test]
    fn a_missing_weekly_window_is_simply_absent() {
        let body = r#"{"rollingUsage":{"usagePercent":10,"resetInSec":60}}"#;
        let snapshot = parse_usage(body, now()).expect("parses");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].label, "5h");
    }

    /// The bound is the whole safety property: without it, a rolling window with no
    /// percentage would silently report the weekly one's.
    #[test]
    fn a_label_cannot_borrow_the_next_windows_figure() {
        let body = format!(
            r#"{{"rollingUsage":{{}}{},"weeklyUsage":{{"usagePercent":99}}}}"#,
            ",\"pad\":\"".to_string() + &"x".repeat(NEAR) + "\""
        );
        assert!(number_near(&body, "rollingUsage", "usagePercent").is_none());
    }

    /// A response that carries neither window is an expired session far more often than an
    /// idle account, so it must not read as 0% used.
    #[test]
    fn no_windows_at_all_is_an_error() {
        assert!(parse_usage(r#"{"subscription":{"plan":"pro"}}"#, now()).is_err());
    }

    /// A countdown of zero or less is a window already turned over, not a reset due now.
    #[test]
    fn a_non_positive_countdown_produces_no_reset_time() {
        let body = r#"{"rollingUsage":{"usagePercent":5,"resetInSec":0}}"#;
        assert!(parse_usage(body, now()).unwrap().windows[0].resets_at.is_none());
    }
}
