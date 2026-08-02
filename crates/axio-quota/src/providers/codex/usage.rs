//! Resolving the usage endpoint and mapping its payload onto a snapshot.

use serde_json::Value;
use time::OffsetDateTime;

use super::credentials::codex_home;
use crate::error::ProbeError;
use crate::json::{as_bool, as_f64, as_i64, as_string, pick};
use crate::model::{Credits, ProviderId, RateWindow, UsageSnapshot};
use crate::paths::Env;

const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";

/// Honours `chatgpt_base_url` in `config.toml`, which is how enterprise users point Codex
/// at a proxy. Getting this wrong sends their bearer token to the wrong host, so it is
/// worth the few lines even though most users never set it.
pub fn usage_url(env: &Env) -> String {
    let configured = read_config_base_url(env).unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let base = normalize_base_url(&configured);
    let path = if base.contains("/backend-api") {
        "/wham/usage"
    } else {
        "/api/codex/usage"
    };
    format!("{base}{path}")
}

fn read_config_base_url(env: &Env) -> Option<String> {
    let raw = std::fs::read_to_string(codex_home(env).join("config.toml")).ok()?;
    parse_base_url_from_toml(&raw)
}

/// Deliberately not a TOML parse: we want one key, and pulling in a TOML dependency to
/// read it would be the larger risk.
fn parse_base_url_from_toml(contents: &str) -> Option<String> {
    for line in contents.lines() {
        let without_comment = line.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = without_comment.split_once('=') else {
            continue;
        };
        if key.trim() != "chatgpt_base_url" {
            continue;
        }
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn normalize_base_url(value: &str) -> String {
    let mut trimmed = value.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        trimmed = DEFAULT_BASE_URL.to_string();
    }
    let is_chatgpt = trimmed.starts_with("https://chatgpt.com")
        || trimmed.starts_with("https://chat.openai.com");
    if is_chatgpt && !trimmed.contains("/backend-api") {
        trimmed.push_str("/backend-api");
    }
    trimmed
}

/// Maps the usage payload onto a snapshot.
///
/// Every field is optional on purpose. This endpoint has gained fields (credits,
/// `individual_limit`, `additional_rate_limits`) over time and will gain more; a strict
/// decode would turn each addition into an outage.
pub fn parse_usage(raw: &str) -> Result<UsageSnapshot, ProbeError> {
    let root: Value =
        serde_json::from_str(raw).map_err(|err| ProbeError::decode("Codex usage response", err))?;

    let mut snapshot = UsageSnapshot::new(ProviderId::Codex);
    snapshot.plan = as_string(pick(&root, &["plan_type", "planType"]));

    if let Some(rate_limit) = pick(&root, &["rate_limit", "rateLimit"]) {
        snapshot
            .windows
            .extend(windows_from_rate_limit(rate_limit, None));
    }

    // Model-scoped limits (e.g. GPT-5.3-Codex-Spark) sit alongside the account-wide
    // windows and use the same shape. One malformed entry must not discard its siblings.
    if let Some(Value::Array(additional)) = pick(&root, &["additional_rate_limits"]) {
        for entry in additional {
            let name = as_string(pick(entry, &["limit_name", "limitName"]));
            let Some(rate_limit) = pick(entry, &["rate_limit", "rateLimit"]) else {
                continue;
            };
            snapshot
                .windows
                .extend(windows_from_rate_limit(rate_limit, name.as_deref()));
        }
    }

    if let Some(credits) = pick(&root, &["credits"]) {
        snapshot.credits = Some(Credits {
            balance: as_f64(credits.get("balance")),
            unlimited: as_bool(credits.get("unlimited")).unwrap_or(false),
            has_credits: as_bool(pick(credits, &["has_credits", "hasCredits"])).unwrap_or(false),
        });
    }

    if snapshot.windows.is_empty() && snapshot.credits.is_none() {
        return Err(ProbeError::decode(
            "Codex usage response",
            "no rate windows or credits in payload",
        ));
    }
    Ok(snapshot)
}

fn windows_from_rate_limit(rate_limit: &Value, scope: Option<&str>) -> Vec<RateWindow> {
    ["primary_window", "secondary_window"]
        .iter()
        .zip(["primaryWindow", "secondaryWindow"])
        .filter_map(|(snake, camel)| window_from(pick(rate_limit, &[snake, camel]), scope))
        .collect()
}

fn window_from(value: Option<&Value>, scope: Option<&str>) -> Option<RateWindow> {
    let value = value?;
    let used = as_f64(pick(value, &["used_percent", "usedPercent"]))?;
    let resets_at = as_i64(pick(value, &["reset_at", "resetAt"]))
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok());
    let window_minutes = as_i64(pick(value, &["limit_window_seconds", "limitWindowSeconds"]))
        .filter(|seconds| *seconds > 0)
        .map(|seconds| (seconds / 60) as u32);

    let label = match scope {
        Some(scope) => format!("{scope} {}", duration_label(window_minutes, "limit")),
        None => duration_label(window_minutes, "Limit"),
    };

    Some(
        RateWindow::new(label, used)
            .with_reset(resets_at)
            .with_window_minutes(window_minutes),
    )
}

/// Names a window by how long it is, not by which JSON field it arrived in.
///
/// `primary_window` is not always the session window: on plans with a single weekly cap it
/// *is* the weekly window and `secondary_window` is null. Labelling by position shows those
/// accounts "5h" next to a countdown six days out.
fn duration_label(minutes: Option<u32>, unknown: &str) -> String {
    match minutes {
        None => unknown.to_string(),
        Some(10_080) => "Weekly".to_string(),
        Some(minutes) if (43_200..=44_640).contains(&minutes) => "Monthly".to_string(),
        Some(minutes) if minutes < 60 => format!("{minutes}m"),
        Some(minutes) if minutes < 1_440 => format!("{}h", minutes / 60),
        Some(minutes) => format!("{}d", minutes / 1_440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_both_windows_and_credits() {
        let raw = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 42,
                    "reset_at": 1767225600,
                    "limit_window_seconds": 18000
                },
                "secondary_window": {
                    "used_percent": 88.5,
                    "reset_at": 1767830400,
                    "limit_window_seconds": 604800
                }
            },
            "credits": { "has_credits": true, "unlimited": false, "balance": "12.5" }
        }"#;
        let snapshot = parse_usage(raw).expect("parses");

        assert_eq!(snapshot.plan.as_deref(), Some("pro"));
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].label, "5h");
        assert_eq!(snapshot.windows[0].window_minutes, Some(300));
        assert_eq!(snapshot.windows[1].window_minutes, Some(10080));
        assert_eq!(snapshot.credits.as_ref().unwrap().balance, Some(12.5));
        // The weekly window is the one in trouble, so it is what the tray should show.
        assert_eq!(snapshot.headline().unwrap().label, "Weekly");
    }

    #[test]
    fn one_malformed_window_does_not_discard_the_other() {
        // The kind of payload that breaks a strict decode: `secondary_window` went from an
        // object to a string in some server-side change. The other window is still good.
        let raw = r#"{
            "rate_limit": {
                "primary_window": { "used_percent": 10, "reset_at": 0 },
                "secondary_window": "unavailable"
            }
        }"#;
        let snapshot = parse_usage(raw).expect("parses");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].used_percent, 10.0);
        assert!(snapshot.windows[0].resets_at.is_none());
    }

    #[test]
    fn a_weekly_primary_window_is_not_labelled_as_a_session_window() {
        // Trimmed from a live Pro account: the only window is weekly and it arrives as
        // `primary_window`, with `secondary_window` null. Labelling by position would call
        // this "5h" and show a six-day countdown next to it.
        let raw = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 21,
                    "limit_window_seconds": 604800,
                    "reset_at": 1786161077
                },
                "secondary_window": null
            },
            "additional_rate_limits": [
                { "limit_name": "GPT-5.3-Codex-Spark",
                  "rate_limit": {
                      "primary_window": {
                          "used_percent": 0,
                          "limit_window_seconds": 604800,
                          "reset_at": 1786256920
                      },
                      "secondary_window": null
                  } }
            ],
            "credits": { "has_credits": false, "unlimited": false, "balance": "0" }
        }"#;
        let snapshot = parse_usage(raw).expect("parses");

        let labels: Vec<_> = snapshot.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Weekly", "GPT-5.3-Codex-Spark Weekly"]);
        assert_eq!(snapshot.windows[0].window_minutes, Some(10_080));
        // The string "0" has to survive as a number, not fail the whole decode.
        assert_eq!(snapshot.credits.as_ref().unwrap().balance, Some(0.0));
        assert_eq!(snapshot.headline().unwrap().label, "Weekly");
    }

    #[test]
    fn window_labels_come_from_duration() {
        assert_eq!(duration_label(Some(300), "Limit"), "5h");
        assert_eq!(duration_label(Some(10_080), "Limit"), "Weekly");
        assert_eq!(duration_label(Some(43_200), "Limit"), "Monthly");
        assert_eq!(duration_label(Some(30), "Limit"), "30m");
        assert_eq!(duration_label(None, "Limit"), "Limit");
    }

    #[test]
    fn an_empty_payload_is_an_error_not_an_empty_snapshot() {
        assert!(parse_usage(r#"{ "rate_limit": {} }"#).is_err());
    }

    #[test]
    fn base_url_override_is_read_from_config_toml() {
        let contents = "model = \"gpt-5\"\nchatgpt_base_url = \"https://proxy.internal/backend-api\"  # prod\n";
        assert_eq!(
            parse_base_url_from_toml(contents).as_deref(),
            Some("https://proxy.internal/backend-api")
        );
    }

    #[test]
    fn chatgpt_hosts_get_the_backend_api_prefix_added() {
        assert_eq!(
            normalize_base_url("https://chatgpt.com/"),
            "https://chatgpt.com/backend-api"
        );
        assert_eq!(
            normalize_base_url("https://proxy.internal/v1"),
            "https://proxy.internal/v1"
        );
    }
}
