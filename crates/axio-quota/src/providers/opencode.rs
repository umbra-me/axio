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
use crate::provider::{FetchContext, Provider, redirect_target};

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
        .and_then(|raw| super::cookie_header_for(ProviderId::Opencode, raw))
        .ok_or_else(|| {
            ProbeError::NotConfigured(
                "No opencode session. Open opencode.ai signed in, copy the Cookie header from \
                 any request in the Network tab, and paste it into Settings."
                    .to_string(),
            )
        })
}

/// The server function that lists the workspaces a session can see.
const WORKSPACES_FN: &str = "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";

/// The first workspace id in a response.
///
/// Scanned for rather than parsed, for the same reason the usage figures are: the response
/// is a serialized object graph in `text/javascript`, not JSON. A workspace id has a
/// distinctive shape — `wrk_` and then base-something — which makes finding one reliable
/// even though the document around it is not worth modelling.
pub fn find_workspace(text: &str) -> Option<String> {
    let at = text.find("wrk_")?;
    let id: String = text[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    // `wrk_` alone is a prefix with nothing after it, which is not an id.
    (id.len() > 4).then_some(id)
}

/// Ask the site which workspace this session belongs to.
async fn discover_workspace(ctx: &FetchContext, session: &str) -> Result<String, ProbeError> {
    let url = format!("{SERVER_URL}?id={WORKSPACES_FN}&args=%5B%5D");
    let response = ctx
        .http
        .get(&url)
        .header("Cookie", session)
        .header("X-Server-Id", WORKSPACES_FN)
        .header("X-Server-Instance", "server-fn:axio")
        .header("Origin", "https://opencode.ai")
        .header("Referer", "https://opencode.ai/")
        .header("Accept", "text/javascript, application/json;q=0.9, */*;q=0.8")
        .send()
        .await
        .map_err(|err| ProbeError::network(PROVIDER, err))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| ProbeError::network(PROVIDER, err))?;

    if !status.is_success() {
        return Err(ProbeError::Unauthorized(format!(
            "opencode answered {status} when asked for your workspaces. Sign in again, or put \
             the workspace id in Settings to skip this lookup."
        )));
    }
    find_workspace(&body).ok_or_else(|| {
        ProbeError::NotConfigured(
            "Signed in, but this account has no workspace the billing page can report. Put a \
             workspace id in Settings if you know it."
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
        // The cookie is checked before anything else, so a missing session says so rather
        // than being reported as a missing workspace — which is what a lookup failure
        // would otherwise look like.
        let session = cookie(ctx)?;

        let configured = ctx
            .config
            .workspace_id
            .as_deref()
            .map(str::trim)
            // A pasted URL is what someone has to hand; the id is the last path segment.
            .map(|raw| raw.rsplit('/').next().unwrap_or(raw))
            .filter(|id| !id.is_empty())
            .map(str::to_string);

        // Asked for rather than asked about. The site knows which workspaces this session
        // can see, and making someone find an id in a URL to tell us something the server
        // will volunteer is a step that exists only because nobody removed it.
        let workspace = match configured {
            Some(id) => id,
            None => discover_workspace(ctx, &session).await?,
        };

        // A GET with the function id in the query, not a POST with it in a header — the
        // server routes on `?id=` and answers anything else with its own error page. The
        // argument list is the JSON array the function takes, which here is just the id.
        //
        // Encoded by hand rather than with a query builder: the three characters that need
        // it are the JSON punctuation, and a workspace id is `wrk_` and hex.
        let args = format!("%5B%22{workspace}%22%5D");
        let url = format!("{SERVER_URL}?id={SUBSCRIPTION_FN}&args={args}");
        let response = ctx
            .http
            .get(&url)
            .header("Cookie", session)
            .header("X-Server-Id", SUBSCRIPTION_FN)
            // The framework tags each call with an instance id. A fixed one is enough —
            // it distinguishes calls, and nothing here makes two at once.
            .header("X-Server-Instance", "server-fn:axio")
            // Origin and Referer are not decoration. This is a browser endpoint, and one
            // that answers a request without them as a cross-site call rather than a
            // session — which looks exactly like a rejected cookie from the outside.
            .header("Origin", "https://opencode.ai")
            .header(
                "Referer",
                format!("https://opencode.ai/workspace/{workspace}/billing"),
            )
            .header("Accept", "text/javascript, application/json;q=0.9, */*;q=0.8")
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
            200..=299 => parse_usage(&body, OffsetDateTime::now_utc()),
            300..=399 | 401 | 403 => Err(ProbeError::Unauthorized(format!(
                "opencode answered {status}{}. Sign in at opencode.ai and paste a fresh Cookie \
                 header, and check the workspace id belongs to that account.",
                location
                    .map(|to| format!(" redirecting to {to}"))
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

    /// The id is found in a document that is not worth modelling, so the shape is the
    /// whole check — a prefix with nothing after it is not an id.
    #[test]
    fn a_workspace_id_is_found_by_its_shape() {
        assert_eq!(
            find_workspace(r#"0:{"workspaces":[{"id":"wrk_01ABCdef","name":"Personal"}]}"#)
                .as_deref(),
            Some("wrk_01ABCdef")
        );
        assert_eq!(find_workspace("no workspace here"), None);
        assert_eq!(find_workspace("wrk_"), None);
    }

    /// The first is taken, and that has to be the first in the document rather than the
    /// first alphabetically — an account with two workspaces should get the one the site
    /// lists first, which is the one its own billing page opens.
    #[test]
    fn the_first_workspace_listed_is_the_one_taken() {
        let body = r#"[{"id":"wrk_zzz"},{"id":"wrk_aaa"}]"#;
        assert_eq!(find_workspace(body).as_deref(), Some("wrk_zzz"));
    }

    /// A countdown of zero or less is a window already turned over, not a reset due now.
    #[test]
    fn a_non_positive_countdown_produces_no_reset_time() {
        let body = r#"{"rollingUsage":{"usagePercent":5,"resetInSec":0}}"#;
        assert!(parse_usage(body, now()).unwrap().windows[0].resets_at.is_none());
    }
}
