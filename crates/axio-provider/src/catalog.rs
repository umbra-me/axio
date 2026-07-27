//! What an endpoint says it serves.
//!
//! Both dialects publish a listing at `/models` and both wrap it the same way —
//! `{"data": [{"id": "..."}]}` — so one parser reads either. Shared rather than
//! written twice, because two parsers for one shape drift and the symptom is a
//! picker that is missing entries on one provider only.

use axio_core::provider::ProviderError;
use axio_core::redact::Redacted;

/// The model names in a listing response.
///
/// Entries without an `id` are skipped rather than failing the whole listing: a
/// provider adding a field is not a reason to show the user nothing, and the
/// names that did parse are still the names they can pick.
pub(crate) fn model_ids(body: &str) -> Result<Vec<String>, ProviderError> {
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::Transport(Redacted::new(format!("model listing: {e}"))))?;

    let Some(entries) = parsed.get("data").and_then(|d| d.as_array()) else {
        return Err(ProviderError::Transport(Redacted::new(
            "model listing had no `data` array".to_owned(),
        )));
    };

    Ok(entries
        .iter()
        .filter_map(|entry| entry.get("id")?.as_str().map(str::to_owned))
        .collect())
}

/// Where a listing lives, given the endpoint the requests go to.
///
/// The Messages dialect is configured with the full path to `/v1/messages`, so
/// the sibling is found by swapping the last segment. Appending would ask for
/// `/v1/messages/models`, which is a 404 that looks like the provider having no
/// listing at all.
pub(crate) fn listing_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    match trimmed.strip_suffix("/messages") {
        Some(root) => format!("{root}/models"),
        None => format!("{trimmed}/models"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_out_of_a_listing() {
        let body = r#"{"data":[{"id":"one","object":"model"},{"id":"two"}]}"#;
        assert_eq!(model_ids(body).unwrap(), vec!["one", "two"]);
    }

    /// A provider adding a field, or an entry shaped differently, must not cost
    /// the user every name that did parse.
    #[test]
    fn an_unreadable_entry_does_not_lose_the_readable_ones() {
        let body = r#"{"data":[{"id":"kept"},{"no_id":true},{"id":42}]}"#;
        assert_eq!(model_ids(body).unwrap(), vec!["kept"]);
    }

    #[test]
    fn a_response_that_is_not_a_listing_is_an_error_not_an_empty_list() {
        // Empty and broken must not look the same: one means "this provider
        // serves nothing", which is never true.
        assert!(model_ids("not json").is_err());
        assert!(model_ids(r#"{"error":"nope"}"#).is_err());
        assert_eq!(model_ids(r#"{"data":[]}"#).unwrap(), Vec::<String>::new());
    }

    /// The regression: appending to the Messages URL asks for
    /// `/v1/messages/models` and gets a 404 that reads as "no listing here".
    #[test]
    fn the_messages_path_becomes_its_sibling() {
        assert_eq!(
            listing_url("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            listing_url("https://ollama.com/v1"),
            "https://ollama.com/v1/models"
        );
        assert_eq!(
            listing_url("https://gateway.internal/v1/"),
            "https://gateway.internal/v1/models"
        );
    }
}
