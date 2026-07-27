//! Turning a failed response into something the loop can act on.
//!
//! The distinction that matters is retryable from fatal: a refusal on a 200 is
//! an outcome, an overflow is a compaction, and a 400 is the end.

use super::*;

/// Classify a non-2xx response.
///
/// `retry_after` is the header value. It is honoured over any local backoff
/// table, because the server knows when it will accept work again and we do not.
pub fn classify(status: u16, retry_after: Option<&str>, body: &str) -> ProviderError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let etype = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(body);

    match status {
        401 | 403 => ProviderError::Auth(Redacted::new(message)),
        429 => ProviderError::RateLimited {
            retry_after: parse_retry_after(retry_after),
        },
        529 => ProviderError::Overloaded,
        400 if is_context_overflow(message) || etype == "context_window_exceeded_error" => {
            ProviderError::ContextOverflow
        }
        _ => ProviderError::Http {
            status,
            body: Redacted::new(summarise_body(body, parsed.is_some())),
        },
    }
}

/// How much of an error body is worth showing.
pub(super) const MAX_BODY_CHARS: usize = 400;

/// A bounded, diagnosable rendering of an error body.
///
/// The body went to the terminal, and on a recorded run to the session file,
/// uncapped. `example.com` returns 576 bytes of HTML; a corporate proxy error
/// page, an SSO login form or a misrouted CDN response is routinely hundreds of
/// kilobytes. And the most useful fact — that the response was not JSON, so the
/// endpoint is probably not an API at all — was the one thing the dump did not
/// say.
pub(super) fn summarise_body(body: &str, was_json: bool) -> String {
    let trimmed = body.trim();
    let mut text: String = trimmed.chars().take(MAX_BODY_CHARS).collect();
    if trimmed.chars().count() > MAX_BODY_CHARS {
        text.push('…');
    }
    if was_json || trimmed.is_empty() {
        text
    } else {
        format!("the response was not JSON, so this endpoint is probably not an API: {text}")
    }
}

/// The message text is the only signal the API gives for an oversize prompt on
/// a 400, so this match is deliberately broad — a false positive costs one
/// compaction pass, a false negative fails the turn.
pub fn is_context_overflow(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    (m.contains("context")
        && (m.contains("exceed") || m.contains("too long") || m.contains("window")))
        || m.contains("prompt is too long")
        || m.contains("too many tokens")
}

pub(super) fn parse_retry_after(header: Option<&str>) -> Option<Duration> {
    let raw = header?.trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    // A float is out of spec but has been observed in the wild.
    raw.parse::<f64>()
        .ok()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .map(Duration::from_secs_f64)
}
