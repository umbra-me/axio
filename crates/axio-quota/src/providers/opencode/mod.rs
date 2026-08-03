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
use time::OffsetDateTime;

mod payload;

pub use payload::{error_message, is_signed_out, parse_usage};
use payload::{dump, signed_out_or_broken};

use crate::error::ProbeError;
use crate::model::{ProviderId, UsageSnapshot};
use crate::paths::Env;
use crate::provider::{FetchContext, Provider, redirect_target};

const PROVIDER: &str = "opencode";
const SERVER_URL: &str = "https://opencode.ai/_server";

/// The server function that returns the subscription's usage windows. opencode addresses
/// its server functions by content hash rather than by name, so this is the name.
const SUBSCRIPTION_FN: &str = "7abeebee372f304e050aaaf92be863f4a86490e382f8c79db68fd94040d691b4";

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
    dump("workspaces", &body);
    // This call fails first when the session is not recognised, so it is where the useful
    // message lives. Without this the failure surfaces as "no workspace", which sends the
    // fix in the wrong direction — it is not the workspace that is missing.
    if let Some(message) = error_message(&body) {
        return Err(signed_out_or_broken(&message));
    }
    find_workspace(&body).ok_or_else(|| {
        ProbeError::NotConfigured(
            "Signed in, but this account has no workspace the billing page can report. Put a \
             workspace id in Settings if you know it."
                .to_string(),
        )
    })
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
            .header("Cookie", &session)
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
            200..=299 => {
                dump("subscription GET", &body);
                if let Ok(snapshot) = parse_usage(&body, OffsetDateTime::now_utc()) {
                    return Ok(snapshot);
                }
                // The same function answers a GET with an empty payload and a POST with the
                // real one, depending on how the framework decided to route it that day.
                // Another client for this endpoint retries the same way rather than
                // treating the empty GET as the answer.
                let posted = post_subscription(ctx, &session, &workspace).await?;
                dump("subscription POST", &posted);
                parse_usage(&posted, OffsetDateTime::now_utc())
            }
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

/// The same call as a POST, with the arguments in the body.
async fn post_subscription(
    ctx: &FetchContext,
    session: &str,
    workspace: &str,
) -> Result<String, ProbeError> {
    let response = ctx
        .http
        .post(format!("{SERVER_URL}?id={SUBSCRIPTION_FN}"))
        .header("Cookie", session)
        .header("X-Server-Id", SUBSCRIPTION_FN)
        .header("X-Server-Instance", "server-fn:axio")
        .header("Origin", "https://opencode.ai")
        .header(
            "Referer",
            format!("https://opencode.ai/workspace/{workspace}/billing"),
        )
        .header("Content-Type", "application/json")
        .header("Accept", "text/javascript, application/json;q=0.9, */*;q=0.8")
        .body(format!("[\"{workspace}\"]"))
        .send()
        .await
        .map_err(|err| ProbeError::network(PROVIDER, err))?;

    response
        .text()
        .await
        .map_err(|err| ProbeError::network(PROVIDER, err))
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
}
