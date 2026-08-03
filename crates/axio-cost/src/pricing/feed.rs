//! Reading a refreshed price feed.
//!
//! Parsing only. Fetching is the caller's job — this crate opens no sockets, so it stays
//! testable without one and usable without one.
//!
//! The shape is models.dev's: provider, then model, then a `cost` object already quoted
//! per million tokens.
//!
//! ```json
//! { "zhipuai": { "models": { "glm-5": { "cost": {
//!     "input": 1, "output": 3.2, "cache_read": 0.2, "cache_write": 0 } } } } }
//! ```
//!
//! # Why the overlay only fills gaps
//!
//! A feed row is four numbers. The bundled table is four numbers *plus structure the feed
//! has nowhere to put*: OpenAI's rate doubling past 272K input, Sonnet 5's introductory
//! window closing on a date, cache ratios checked against each vendor's own page. Letting
//! a flat row replace a tiered one would silently drop the tier and quietly under-bill
//! every long request.
//!
//! So the feed is authoritative for models the bundle has never heard of, which is the
//! problem it exists to solve, and the bundle keeps the ones it has verified. A price that
//! goes stale in the bundle is a visible edit to a table with a cited source; a tier lost
//! to an overlay is invisible.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{ModelPricing, normalize};

#[derive(Deserialize)]
struct Provider {
    #[serde(default)]
    models: BTreeMap<String, Model>,
}

#[derive(Deserialize)]
struct Model {
    #[serde(default)]
    cost: Option<Cost>,
}

#[derive(Deserialize)]
struct Cost {
    #[serde(default)]
    input: Option<f64>,
    #[serde(default)]
    output: Option<f64>,
    #[serde(default)]
    cache_read: Option<f64>,
    #[serde(default)]
    cache_write: Option<f64>,
}

/// Parse a feed document into rates, keyed by normalized model id.
///
/// The same model appears under several providers — a first-party vendor and any number
/// of gateways reselling it — at different prices. Where they disagree the **dearest** is
/// kept, on the same principle applied everywhere else here: a figure that is too high is
/// visible and arguable, one that is too low is neither.
///
/// A model with no `input` price is skipped rather than admitted at zero. Free-tier and
/// not-yet-priced entries are common in these feeds and both look like `0`.
pub fn parse(document: &str) -> Result<Vec<(String, ModelPricing)>, serde_json::Error> {
    let providers: BTreeMap<String, Provider> = serde_json::from_str(document)?;

    let mut best: BTreeMap<String, ModelPricing> = BTreeMap::new();
    for provider in providers.values() {
        for (id, model) in &provider.models {
            let Some(cost) = &model.cost else { continue };
            let Some(input) = cost.input.filter(|input| *input > 0.0) else {
                continue;
            };
            let Some(output) = cost.output.filter(|output| *output > 0.0) else {
                continue;
            };

            // An absent cache read is not a free one. Falling back to the input rate
            // charges cache hits as ordinary input, which overstates rather than invents
            // a discount the vendor may not offer.
            let cache_read = cost.cache_read.unwrap_or(input);
            // A cache write of zero is taken at face value: several vendors genuinely
            // populate their cache as a side effect and charge nothing extra for it.
            let cache_write = cost.cache_write.unwrap_or(input);

            let rates = ModelPricing {
                input,
                output,
                cache_read,
                cache_write_5m: cache_write,
                cache_write_1h: cache_write,
                long_context: None,
            };

            best.entry(normalize(id))
                .and_modify(|existing| {
                    if rates.input > existing.input {
                        *existing = rates;
                    }
                })
                .or_insert(rates);
        }
    }

    Ok(best.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"{
      "zhipuai": { "id": "zhipuai", "models": {
        "glm-5": { "id": "glm-5", "cost": {"input": 1, "output": 3.2, "cache_read": 0.2, "cache_write": 0} }
      }},
      "gateway": { "id": "gateway", "models": {
        "glm-5": { "id": "glm-5", "cost": {"input": 0.5, "output": 1.6, "cache_read": 0.1, "cache_write": 0} },
        "brand-new-model": { "id": "brand-new-model", "cost": {"input": 7, "output": 21} }
      }}
    }"#;

    #[test]
    fn a_feed_becomes_rates_keyed_by_normalized_model_id() {
        let rates: BTreeMap<_, _> = parse(FEED).expect("parses").into_iter().collect();
        assert!(rates.contains_key("glm-5"));
        assert!(rates.contains_key("brand-new-model"));
    }

    /// The same model resold at two prices keeps the dearer, for the same reason a
    /// total-only token count is costed as output: too high is arguable, too low is not.
    #[test]
    fn the_dearest_quote_for_a_model_wins() {
        let rates: BTreeMap<_, _> = parse(FEED).expect("parses").into_iter().collect();
        assert_eq!(rates["glm-5"].input, 1.0);
        assert_eq!(rates["glm-5"].output, 3.2);
    }

    #[test]
    fn a_published_cache_read_is_used_verbatim() {
        let rates: BTreeMap<_, _> = parse(FEED).expect("parses").into_iter().collect();
        assert_eq!(rates["glm-5"].cache_read, 0.2, "not a tenth of input");
        assert_eq!(rates["glm-5"].cache_write_5m, 0.0);
    }

    /// An absent cache read must not become a free one.
    #[test]
    fn a_missing_cache_rate_falls_back_to_the_input_rate() {
        let rates: BTreeMap<_, _> = parse(FEED).expect("parses").into_iter().collect();
        let new = rates["brand-new-model"];
        assert_eq!(new.cache_read, 7.0);
        assert_eq!(new.cache_write_5m, 7.0);
    }

    /// Zero and "not yet priced" are indistinguishable in these feeds, so neither is
    /// admitted — the model stays unpriced and says so.
    #[test]
    fn a_zero_or_absent_price_is_skipped_rather_than_admitted() {
        let feed = r#"{"p":{"models":{
          "free-tier":{"cost":{"input":0,"output":0}},
          "no-cost-block":{"id":"no-cost-block"},
          "output-only":{"cost":{"output":5}}
        }}}"#;
        assert!(parse(feed).expect("parses").is_empty());
    }

    #[test]
    fn a_provider_with_no_models_is_not_an_error() {
        assert!(parse(r#"{"empty":{"id":"empty"}}"#).expect("parses").is_empty());
    }

    #[test]
    fn a_malformed_document_is_an_error_rather_than_an_empty_table() {
        assert!(parse("not json").is_err());
    }

    /// End to end: a feed fills a gap without disturbing a bundled row.
    #[test]
    fn an_overlay_fills_gaps_and_leaves_verified_rows_alone() {
        let prices = super::super::Prices::bundled()
            .with_overlay("models.dev", parse(FEED).expect("parses"));

        // Newly known, from the feed.
        assert!(prices.resolve("brand-new-model", "2026-08-03").pricing.is_some());

        // Bundled and tiered: the feed has no way to express the 272K step, so the
        // bundled row must survive.
        let sol = prices
            .resolve("gpt-5.6-sol", "2026-08-03")
            .pricing
            .expect("bundled");
        assert!(sol.long_context.is_some(), "the tier survived the overlay");
    }
}
