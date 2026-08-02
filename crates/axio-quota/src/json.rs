//! Lenient JSON accessors.
//!
//! Provider APIs change shape without warning, send the same field as a number in one
//! response and a string in the next, and rename keys between snake_case and camelCase.
//! Deriving `Deserialize` on the whole payload means one unexpected field discards the
//! entire response — including the fields that were fine.
//!
//! So probes parse into [`serde_json::Value`] and pick fields out with these helpers. A
//! field we cannot understand becomes `None`; it never fails the surrounding parse.

use serde_json::Value;

/// First present, non-null value among `keys`.
///
/// Handles both snake/camel duality (`resets_at` vs `resetsAt`) and providers that rename
/// a field across versions while still serving the old one to some accounts.
pub fn pick<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| value.get(*key).filter(|found| !found.is_null()))
}

/// A number that may arrive as a JSON number or as a stringified number.
pub fn as_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// An integer that may arrive as an int, a float, or a stringified either.
pub fn as_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => {
            let trimmed = text.trim();
            trimmed
                .parse::<i64>()
                .ok()
                .or_else(|| trimmed.parse::<f64>().ok().map(|float| float as i64))
        }
        _ => None,
    }
}

/// A non-empty, trimmed string. Empty strings are treated as absent, because providers
/// use `""` and `null` interchangeably for "no value".
pub fn as_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        _ => None,
    }
}

/// A bool that may arrive as a bool, as 0/1, or as "true"/"false".
pub fn as_bool(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number.as_i64().map(|int| int != 0),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_survive_being_sent_as_strings() {
        let payload = json!({ "limit": "42.5", "used": 7, "resets_at": 1_700_000_000_i64 });
        assert_eq!(as_f64(payload.get("limit")), Some(42.5));
        assert_eq!(as_f64(payload.get("used")), Some(7.0));
        assert_eq!(as_i64(payload.get("resets_at")), Some(1_700_000_000));
    }

    #[test]
    fn pick_prefers_the_first_present_key() {
        let payload = json!({ "remainingPercent": 30 });
        assert_eq!(
            as_f64(pick(&payload, &["remaining_percent", "remainingPercent"])),
            Some(30.0)
        );
        assert!(pick(&payload, &["absent", "alsoAbsent"]).is_none());
    }

    #[test]
    fn explicit_null_counts_as_absent() {
        let payload = json!({ "plan": null, "fallback": "pro" });
        assert_eq!(
            as_string(pick(&payload, &["plan", "fallback"])),
            Some("pro".to_string())
        );
    }
}
