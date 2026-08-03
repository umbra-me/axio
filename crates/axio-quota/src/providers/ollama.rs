//! Ollama Cloud usage.
//!
//! The awkward one, and the awkwardness is Ollama's: the documented API verifies a key but
//! does not expose the Cloud Usage windows at all. Those exist only on the settings page,
//! as rendered HTML. So this scrapes, and scraping is a promise about someone else's markup
//! that they never made.
//!
//! What that buys is the discipline in [`parse_settings`]: it looks for the two labelled
//! figures and the timestamps beside them, and returns an error the moment the shape it
//! expects is not there. A scraper that guesses when the page changes reports a number that
//! is not usage, which is worse than reporting nothing — the page will change.

use async_trait::async_trait;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::ProbeError;
use crate::model::{ProviderId, RateWindow, UsageSnapshot};
use crate::paths::{Env, non_empty};
use crate::provider::{FetchContext, Provider, redirect_target};

const PROVIDER: &str = "Ollama";
const SETTINGS_URL: &str = "https://ollama.com/settings";
const API_KEY_ENV: &str = "OLLAMA_API_KEY";

pub struct OllamaProvider;

fn cookie(ctx: &FetchContext) -> Result<String, ProbeError> {
    ctx.config
        .cookie_header
        .as_deref()
        .and_then(|raw| super::cookie_header_for(ProviderId::Ollama, raw))
        .ok_or_else(|| {
            ProbeError::NotConfigured(
                "No Ollama session. The Cloud Usage bars are not on the API — open \
                 ollama.com/settings signed in, copy the Cookie header from the Network tab, \
                 and paste it into Settings."
                    .to_string(),
            )
        })
}

/// A percentage written as `42%`, or as `42.5%`.
fn percent_near(html: &str, label: &str) -> Option<f64> {
    let at = html.find(label)?;
    // Bounded so a missing figure cannot walk the rest of the document and pick up an
    // unrelated percentage from further down the page.
    let window = &html[at..html.len().min(at + 2_000)];
    let percent = window.find('%')?;
    let digits: String = window[..percent]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.chars().rev().collect::<String>().parse().ok()
}

/// The `data-time` attribute beside a "Resets in …" element.
fn reset_near(html: &str, label: &str) -> Option<OffsetDateTime> {
    let at = html.find(label)?;
    let window = &html[at..html.len().min(at + 2_000)];
    let key = window.find("data-time=")?;
    // The attribute value is quoted, so it ends at the closing quote rather than at the
    // next `<` — the tag's own text comes after it and would otherwise be swallowed.
    let after = &window[key + "data-time=".len()..];
    let quote = after.chars().next()?;
    let value = after.strip_prefix(quote)?.split(quote).next()?;
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

/// Pull the plan badge and the two usage bars out of the settings page.
pub fn parse_settings(html: &str) -> Result<UsageSnapshot, ProbeError> {
    // A signed-out request is answered with the sign-in page, status 200. Detecting that
    // here is what turns "your session expired" into a message rather than a parse error.
    if html.contains("/signin") && !html.contains("Cloud Usage") {
        return Err(ProbeError::Unauthorized(
            "Ollama redirected to sign-in. Paste a fresh Cookie header.".to_string(),
        ));
    }

    let mut snapshot = UsageSnapshot::new(ProviderId::Ollama);
    snapshot.plan = ["Free", "Pro", "Max"]
        .into_iter()
        .find(|tier| html.contains(&format!(">{tier}<")))
        .map(str::to_string);

    for (label, name) in [("Session usage", "Session"), ("Weekly usage", "Weekly")] {
        if let Some(used) = percent_near(html, label) {
            snapshot
                .windows
                .push(RateWindow::new(name, used).with_reset(reset_near(html, label)));
        }
    }

    if snapshot.windows.is_empty() {
        return Err(ProbeError::decode(
            "Ollama settings page",
            "no Cloud Usage bars found — the page layout may have changed",
        ));
    }
    Ok(snapshot)
}

#[async_trait]
impl Provider for OllamaProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn credential_hint(&self, _env: &Env) -> String {
        format!("config cookieHeader for usage; ${API_KEY_ENV} verifies a key only")
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        // The API key is accepted and stored, but it cannot answer this question — saying
        // so is better than letting someone set one and wonder why the bars stay empty.
        let _ = non_empty(&ctx.env, API_KEY_ENV);

        let response = ctx
            .http
            .get(SETTINGS_URL)
            .header("Cookie", cookie(ctx)?)
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Referer", "https://ollama.com/")
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
            200..=299 => parse_settings(&body),
            // A redirect is how an expired session usually arrives here, and
            // `Policy::none` leaves it to be read rather than followed.
            300..=399 | 401 | 403 => Err(ProbeError::Unauthorized(format!(
                "Ollama answered {status}{}. The header needs the session cookie — on the \
                 current site that is `wos-session`.",
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

    const PAGE: &str = r#"
      <div class="badge"><span>Cloud Usage</span><span>Pro</span></div>
      <div><h3>Session usage</h3><span>36%</span>
        <span data-time="2026-08-03T18:00:00Z">Resets in 4 hours</span></div>
      <div><h3>Weekly usage</h3><span>7.5%</span>
        <span data-time="2026-08-09T00:00:00Z">Resets in 6 days</span></div>
    "#;

    #[test]
    fn the_two_bars_and_the_plan_are_read_from_the_page() {
        let snapshot = parse_settings(PAGE).expect("parses");
        assert_eq!(snapshot.plan.as_deref(), Some("Pro"));
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].label, "Session");
        assert_eq!(snapshot.windows[0].used_percent, 36.0);
        assert_eq!(snapshot.windows[1].used_percent, 7.5);
        assert!(snapshot.windows[0].resets_at.is_some());
    }

    /// The search window is bounded, so a bar with no figure cannot borrow one from an
    /// unrelated percentage further down the document.
    #[test]
    fn a_missing_figure_is_not_taken_from_elsewhere_on_the_page() {
        let page = format!(
            "<h3>Session usage</h3><div>no figure here</div>{}<span>99%</span>",
            "x".repeat(3_000)
        );
        assert!(percent_near(&page, "Session usage").is_none());
    }

    /// The page will change. When it does this must fail loudly rather than report a
    /// number that is not usage.
    #[test]
    fn an_unrecognised_page_is_an_error_rather_than_a_guess() {
        assert!(parse_settings("<html><body>Something else entirely</body></html>").is_err());
    }

    /// A signed-out request is answered with the sign-in page at status 200, so the status
    /// code alone cannot tell you the session died.
    #[test]
    fn the_sign_in_page_is_recognised_as_an_expired_session() {
        let page = r#"<html><a href="/signin">Sign in</a></html>"#;
        assert!(matches!(
            parse_settings(page),
            Err(ProbeError::Unauthorized(_))
        ));
    }
}
