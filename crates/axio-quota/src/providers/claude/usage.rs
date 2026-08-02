//! Mapping the OAuth usage payload onto a snapshot.

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::ProbeError;
use crate::json::{as_f64, as_string, pick};
use crate::model::{ProviderId, RateWindow, UsageSnapshot};

/// The well-known flat keys, whose labels users recognise.
const KNOWN_WINDOWS: [(&str, &str, Option<u32>); 4] = [
    ("five_hour", "5h", Some(300)),
    ("seven_day", "Weekly", Some(10_080)),
    ("seven_day_opus", "Weekly (Opus)", Some(10_080)),
    ("seven_day_sonnet", "Weekly (Sonnet)", Some(10_080)),
];

/// Three shapes coexist in one payload: the flat keys above, a row per model tier under
/// whatever name that tier ships as, and a `limits` array whose entries carry a display
/// name for the model they scope to. All three are read, and the first label to claim a
/// window keeps it.
pub fn parse_usage(raw: &str) -> Result<UsageSnapshot, ProbeError> {
    let root: Value = serde_json::from_str(raw)
        .map_err(|err| ProbeError::decode("Claude usage response", err))?;

    let mut snapshot = UsageSnapshot::new(ProviderId::Claude);

    for (key, label, minutes) in KNOWN_WINDOWS {
        if let Some(window) = window_from(root.get(key), label, minutes) {
            snapshot.windows.push(window);
        }
    }

    // Every other window-shaped key. The payload carries a row per model tier and the set
    // changes without notice — a live account currently returns eleven of them, all null.
    // Sweeping by shape means a tier that starts reporting shows up on its own, and no
    // list of names has to be chased in this file. A key only becomes a row once the
    // account actually has a number for it.
    if let Value::Object(fields) = &root {
        for (key, value) in fields {
            if KNOWN_WINDOWS.iter().any(|(known, ..)| known == key) {
                continue;
            }
            // `utilization` specifically, not `percent`: `spend` carries a percent and is
            // a balance, not a window.
            if as_f64(value.get("utilization")).is_none() {
                continue;
            }
            let label = label_from_key(key);
            if snapshot.windows.iter().any(|window| window.label == label) {
                continue;
            }
            if let Some(window) = window_from(Some(value), &label, None) {
                snapshot.windows.push(window);
            }
        }
    }

    if let Some(Value::Array(limits)) = root.get("limits") {
        for entry in limits {
            if let Some(window) = limit_entry_window(entry, &snapshot) {
                snapshot.windows.push(window);
            }
        }
    }

    if snapshot.windows.is_empty() {
        return Err(ProbeError::decode(
            "Claude usage response",
            "no usage windows in payload",
        ));
    }
    Ok(snapshot)
}

/// Turns a payload key into a label: `seven_day_<tier>` reads as a weekly window scoped to
/// that tier, anything else is title-cased as-is.
fn label_from_key(key: &str) -> String {
    if let Some(rest) = key.strip_prefix("seven_day_") {
        return format!("Weekly ({})", title_words(rest));
    }
    if let Some(rest) = key.strip_prefix("five_hour_") {
        return format!("5h ({})", title_words(rest));
    }
    title_words(key)
}

fn title_words(raw: &str) -> String {
    raw.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn window_from(value: Option<&Value>, label: &str, minutes: Option<u32>) -> Option<RateWindow> {
    let value = value?;
    let utilization = as_f64(pick(value, &["utilization", "percent"]))?;
    let resets_at = parse_reset(as_string(pick(value, &["resets_at", "resetsAt"])));
    Some(
        RateWindow::new(label, utilization)
            .with_reset(resets_at)
            .with_window_minutes(minutes),
    )
}

fn limit_entry_window(entry: &Value, snapshot: &UsageSnapshot) -> Option<RateWindow> {
    // `is_active` is deliberately NOT a filter. It marks which limit is currently the
    // governing one, not which limits exist: a live account returns `weekly_all` with
    // `is_active: false` while the flat `seven_day` key reports real usage for that same
    // window. Filtering on it hid every model-scoped entry — which is the whole reason
    // the array is worth reading.
    let percent = as_f64(pick(entry, &["percent", "utilization"]))?;

    let group = as_string(pick(entry, &["group"])).unwrap_or_else(|| "Limit".to_string());
    let model = entry
        .get("scope")
        .and_then(|scope| scope.get("model"))
        .and_then(|model| as_string(pick(model, &["display_name", "displayName"])));
    let base_label = canonical_label(&group);
    let label = match model {
        Some(model) => format!("{base_label} ({model})"),
        None => base_label,
    };

    // The flat keys are the authoritative shape where both are served. Accounts mid-rollout
    // get both, and the same window arrives twice under different names — `five_hour` and a
    // `limits` entry with `group: "session"`. Comparing raw group names would miss that and
    // show the user the same window twice.
    if snapshot.windows.iter().any(|window| window.label == label) {
        return None;
    }

    let minutes = match group.as_str() {
        "weekly" => Some(10_080),
        "five_hour" | "session" => Some(300),
        _ => None,
    };
    Some(
        RateWindow::new(label, percent)
            .with_reset(parse_reset(as_string(pick(
                entry,
                &["resets_at", "resetsAt"],
            ))))
            .with_window_minutes(minutes),
    )
}

/// Collapses the `limits` array's group names onto the labels the flat keys already use,
/// so the same window under two names dedupes to one row.
fn canonical_label(group: &str) -> String {
    match group {
        "five_hour" | "session" => "5h".to_string(),
        "weekly" | "seven_day" => "Weekly".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => "Limit".to_string(),
            }
        }
    }
}

fn parse_reset(raw: Option<String>) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(&raw?, &Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_flat_window_shape() {
        let raw = r#"{
            "five_hour": { "utilization": 12.5, "resets_at": "2026-08-02T18:00:00Z" },
            "seven_day": { "utilization": 64, "resets_at": "2026-08-05T00:00:00Z" },
            "seven_day_opus": { "utilization": 91, "resets_at": "2026-08-05T00:00:00Z" }
        }"#;
        let snapshot = parse_usage(raw).expect("parses");
        assert_eq!(snapshot.windows.len(), 3);
        assert_eq!(snapshot.windows[0].label, "5h");
        assert_eq!(snapshot.windows[0].window_minutes, Some(300));
        assert_eq!(snapshot.headline().unwrap().label, "Weekly (Opus)");
    }

    #[test]
    fn a_model_scoped_window_appears_even_when_it_is_not_the_governing_limit() {
        // Trimmed from a live Max account. Note `is_active: false` on both the weekly
        // entries: the session window is the one currently governing, but `weekly_all`
        // still reports 2% and the flat `seven_day` key agrees. Treating `is_active` as
        // "does this limit exist" therefore hides real windows — including the scoped one,
        // which is the only place a model tier's own allowance is reported.
        let raw = r#"{
            "five_hour": { "utilization": 7.0, "resets_at": "2026-08-02T10:30:00.559673+00:00" },
            "seven_day": { "utilization": 2.0, "resets_at": "2026-08-09T01:00:00.559691+00:00" },
            "seven_day_opus": null,
            "limits": [
                { "kind": "session", "group": "session", "percent": 7,
                  "resets_at": "2026-08-02T18:30:00.559673+08:00", "scope": null,
                  "is_active": true },
                { "kind": "weekly_all", "group": "weekly", "percent": 2,
                  "resets_at": "2026-08-09T09:00:00.559691+08:00", "scope": null,
                  "is_active": false },
                { "kind": "weekly_scoped", "group": "weekly", "percent": 0,
                  "resets_at": null,
                  "scope": { "model": { "id": null, "display_name": "Fable" } },
                  "is_active": false }
            ]
        }"#;
        let snapshot = parse_usage(raw).expect("parses");
        let labels: Vec<_> = snapshot.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Weekly", "Weekly (Fable)"]);
        // A null flat key is absent, not a zero.
        assert!(!labels.contains(&"Weekly (Opus)"));
    }

    #[test]
    fn an_unlisted_model_tier_is_swept_in_by_shape() {
        // Tiers arrive under names this file has never heard of. One that reports a number
        // should show up without an edit here; one reporting null should stay absent.
        let raw = r#"{
            "five_hour": { "utilization": 5 },
            "seven_day_something_new": { "utilization": 33, "resets_at": "2026-08-09T00:00:00Z" },
            "another_tier": null,
            "spend": { "percent": 80, "enabled": false },
            "extra_usage": { "utilization": null, "is_enabled": false }
        }"#;
        let snapshot = parse_usage(raw).expect("parses");
        let labels: Vec<_> = snapshot.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Weekly (Something New)"]);
        // `spend` carries a percent but is a balance, not a window — it must not become one.
        assert!(!labels.iter().any(|label| label.contains("Spend")));
    }

    #[test]
    fn the_same_window_under_two_names_is_shown_once() {
        // What a live account mid-rollout actually returns: the 5h window arrives both as
        // the flat `five_hour` key and as a `limits` entry with `group: "session"`.
        let raw = r#"{
            "five_hour": { "utilization": 4, "resets_at": "2026-08-02T10:30:00Z" },
            "seven_day": { "utilization": 1 },
            "limits": [
                { "group": "session", "percent": 4, "resets_at": "2026-08-02T10:30:00Z" }
            ]
        }"#;
        let snapshot = parse_usage(raw).expect("parses");
        let labels: Vec<_> = snapshot.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Weekly"]);
    }
}
