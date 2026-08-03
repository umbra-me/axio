//! Reading a response that is not a documented format.
//!
//! opencode's server functions answer with JavaScript, not JSON. A success is a serialized
//! object graph and a failure is source that constructs an `Error` and assigns it into the
//! page's pending-result table — both with HTTP 200. Nothing here can be handed to serde,
//! so every function is a way of coping with that: find a number near a label, find the
//! error text, or report the field names when neither works.
//!
//! Split from the provider when the file outgrew the length gate, along the line that was
//! already there — this half talks to nobody.

use crate::error::ProbeError;
use crate::model::{ProviderId, RateWindow, UsageSnapshot};
use time::{Duration, OffsetDateTime};

/// How far past a label to look for its numbers. Wide enough for the fields between them,
/// narrow enough that a missing figure cannot borrow the next window's.
const NEAR: usize = 400;

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

/// What a window's percentage might be called.
///
/// A list rather than a name because the response is not a documented API — it is whatever
/// the site's own server function returns this week. Spelling it one way and failing on the
/// others turns a working session into "no rolling usage found", which is what happened.
const PERCENT_KEYS: &[&str] = &[
    "usagePercent",
    "usedPercent",
    "percentUsed",
    "usage_percent",
    "used_percent",
    "utilizationPercent",
    "utilization",
    "percent",
];

/// What a countdown to the reset might be called.
const RESET_KEYS: &[&str] = &[
    "resetInSec",
    "resetInSeconds",
    "resetSeconds",
    "reset_in_sec",
    "reset_sec",
    "resetsInSec",
    "resetIn",
    "resetSec",
];

/// What each window might be called.
const ROLLING_LABELS: &[&str] = &["rollingUsage", "rolling_usage", "rolling"];
const WEEKLY_LABELS: &[&str] = &["weeklyUsage", "weekly_usage", "weekly"];

/// The first of `fields` that appears near any of `labels`.
fn first_number(text: &str, labels: &[&str], fields: &[&str]) -> Option<f64> {
    labels
        .iter()
        .find_map(|label| fields.iter().find_map(|field| number_near(text, label, field)))
}

/// Log a response's shape when `AXIO_OPENCODE_DUMP` is set.
///
/// Redacted rather than raw. These payloads can carry an account's email and its session
/// identifiers, and the thing actually needed to write a parser is the *structure* — the
/// punctuation and the short field names. So long alphanumeric runs and anything holding an
/// `@` are masked, and what survives is the shape.
pub(super) fn dump(label: &str, text: &str) {
    if std::env::var_os("AXIO_OPENCODE_DUMP").is_none() {
        return;
    }
    let redacted: String = text
        .split_inclusive(|c: char| !c.is_ascii_alphanumeric())
        .map(|piece| {
            let word: String = piece.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            let tail: String = piece.chars().filter(|c| !c.is_ascii_alphanumeric()).collect();
            if word.len() > 24 || piece.contains('@') {
                format!("<{}>{tail}", word.len())
            } else {
                format!("{word}{tail}")
            }
        })
        .collect();
    eprintln!(
        "axio opencode {label}: {} bytes\n{}",
        text.len(),
        redacted.chars().take(1200).collect::<String>()
    );
}

/// The message inside a serialized `new Error("…")`, if the response is one.
///
/// These endpoints answer failures with JavaScript rather than a status code: a 200 whose
/// body constructs an `Error` and assigns it into the page's pending-result table. The
/// transport reports success, the parser finds no usage, and the actual explanation — which
/// is sitting right there in the payload — reaches nobody.
pub fn error_message(text: &str) -> Option<String> {
    let at = text.find("new Error(\"")?;
    let rest = &text[at + "new Error(\"".len()..];
    let mut message = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            // The payload is JavaScript source, so a quote inside arrives escaped.
            '\\' => message.push(chars.next()?),
            '"' => return (!message.is_empty()).then_some(message),
            _ => message.push(c),
        }
    }
    None
}

/// Whether an error means "you are not signed in" rather than "something broke".
///
/// opencode phrases it as an actor type: an unauthenticated caller is the `public` actor,
/// and the complaint is that it has no account. Recognising that phrasing is what turns an
/// inscrutable parse failure into "sign in".
pub fn is_signed_out(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("not associated with an account")
        || lowered.contains("unauthorized")
        || lowered.contains("unauthenticated")
}

/// Render a server-side error as the thing to do about it.
pub(super) fn signed_out_or_broken(message: &str) -> ProbeError {
    if is_signed_out(message) {
        ProbeError::Unauthorized(
            "opencode does not recognise the session. Use Sign in to opencode in Settings —              the cookie it needs is `auth`, and a value copied from anywhere else will not              do."
                .to_string(),
        )
    } else {
        ProbeError::decode("opencode response", message.to_string())
    }
}

/// The identifier-like keys in a document, for an error that says what it saw.
///
/// Names only. This runs when parsing has already failed, which is exactly when the useful
/// thing is what the response *does* contain — and exactly when nobody can see it.
fn keys_in(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (index, _) in text.match_indices('"') {
        let rest = &text[index + 1..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        // A key is a quoted name followed by a colon, allowing for a closing quote.
        if name.is_empty() || name.len() > 40 {
            continue;
        }
        let after = &rest[name.len()..];
        if after.starts_with("\":") && !keys.contains(&name) {
            keys.push(name);
        }
    }
    keys.truncate(40);
    keys
}

/// Lift the usage windows out of the serialized response.
///
/// `now` is a parameter because the response gives a countdown rather than a timestamp:
/// resets arrive as seconds remaining, and turning that into a time needs a clock the test
/// can also hold.
pub fn parse_usage(text: &str, now: OffsetDateTime) -> Result<UsageSnapshot, ProbeError> {
    let mut snapshot = UsageSnapshot::new(ProviderId::Opencode);

    for (labels, name, minutes) in [
        (ROLLING_LABELS, "5h", Some(300u32)),
        (WEEKLY_LABELS, "Weekly", Some(10_080)),
    ] {
        let Some(used) = first_number(text, labels, PERCENT_KEYS) else {
            continue;
        };
        let resets_at = first_number(text, labels, RESET_KEYS)
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
        if let Some(message) = error_message(text) {
            return Err(signed_out_or_broken(&message));
        }
        let keys = keys_in(text);
        return Err(ProbeError::decode(
            "opencode subscription response",
            if keys.is_empty() {
                format!("no usage in a {}-byte response with no field names", text.len())
            } else {
                format!("no usage found. The response has: {}", keys.join(", "))
            },
        ));
    }
    Ok(snapshot)
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
    fn a_window_is_found_under_an_alternative_spelling() {
        let body = r#"{"rolling":{"percentUsed":33,"resetIn":7200}}"#;
        let snapshot = parse_usage(body, now()).expect("parses");
        assert_eq!(snapshot.windows[0].used_percent, 33.0);
        assert_eq!(
            snapshot.windows[0].resets_at,
            Some(now() + Duration::hours(2))
        );
    }

    /// When parsing fails the useful information is what the response *did* contain, and
    /// that is exactly the moment nobody can see it.
    #[test]
    fn a_failure_names_the_fields_it_saw() {
        let body = r#"{"error":"unauthorized","requestId":"abc"}"#;
        let message = parse_usage(body, now()).unwrap_err().to_string();
        assert!(message.contains("error"), "{message}");
        assert!(message.contains("requestId"), "{message}");
    }

    /// Values must never be reported, only names — the response may name an account.
    #[test]
    fn only_key_names_are_reported() {
        let keys = keys_in(r#"{"email":"someone@example.com","plan":"pro"}"#);
        assert_eq!(keys, vec!["email", "plan"]);
        assert!(!keys.iter().any(|key| key.contains("example.com")));
    }

    /// The real payload that sent this in circles: a 200 whose body is JavaScript that
    /// constructs an Error. The transport says success and the explanation is inside.
    #[test]
    fn an_error_is_read_out_of_a_javascript_payload() {
        let body = r#";0x00000266;((self.$R=self.$R||{})["server-fn:axio"]=[],($R=>$R[0]=Object.assign(new Error("actor of type \"public\" is not associated with an account"),{stack:"..."}))($R["server-fn:axio"]))"#;
        let message = error_message(body).expect("an error message");
        assert!(message.contains("not associated with an account"), "{message}");
        assert!(is_signed_out(&message));

        // And it reaches the caller as something to do, not as a parse failure.
        let err = parse_usage(body, now()).unwrap_err().to_string();
        assert!(err.to_lowercase().contains("sign in"), "{err}");
    }

    /// A response that is genuinely usage must not be mistaken for an error.
    #[test]
    fn a_real_payload_carries_no_error() {
        assert_eq!(error_message(BODY), None);
        assert!(!is_signed_out("weekly usage is 8 percent"));
    }

    /// A countdown of zero or less is a window already turned over, not a reset due now.
    #[test]
    fn a_non_positive_countdown_produces_no_reset_time() {
        let body = r#"{"rollingUsage":{"usagePercent":5,"resetInSec":0}}"#;
        assert!(parse_usage(body, now()).unwrap().windows[0].resets_at.is_none());
    }
}
