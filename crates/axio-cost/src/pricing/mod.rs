//! What a model charges, and what a bundle of tokens therefore cost.
//!
//! Two layers, in priority order:
//!
//! 1. an **overlay** refreshed from a public price feed and cached on disk, when the user
//!    has opted in and a refresh has succeeded;
//! 2. the **bundled table** in [`table`], which is compiled in and always available.
//!
//! The bundled layer is the floor rather than a fallback of last resort. A quota tool that
//! reports nothing until it has reached the network is a tool that reports nothing on the
//! aeroplane, and a first run with no cache would otherwise show every session at $0.00 —
//! which reads as *cheap*, not as *unknown*.

mod table;

pub use table::normalize;

use serde::{Deserialize, Serialize};

use crate::tokens::TokenBreakdown;

/// Anthropic derives every cache rate from the input price by a fixed multiplier.
///
/// Published as: cache reads cost ~0.1x base input, a 5-minute cache write 1.25x, and a
/// 1-hour write 2x. Deriving rather than transcribing means a price change is one number
/// per model, and the ratios cannot drift out of step with the input price they describe.
const CACHE_READ: f64 = 0.10;
const CACHE_WRITE_5M: f64 = 1.25;
const CACHE_WRITE_1H: f64 = 2.00;

/// Dollars per million tokens, per bucket.
///
/// Per-million rather than per-token because that is the unit every vendor publishes, and
/// a table written in the published unit can be proofread against the published page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    /// Rates that replace the above for a *single request* whose input exceeds a
    /// threshold. OpenAI charges roughly double past 272K tokens; Anthropic's current
    /// models do not tier, so this is `None` for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_context: Option<LongContext>,
}

/// The dearer rates a long request is billed at, and where they begin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LongContext {
    /// Input tokens — fresh plus cached — above which these rates apply.
    pub above: u64,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
}

/// Build the four Anthropic rates from an input and output price.
pub(super) fn anthropic_rates(input: f64, output: f64) -> ModelPricing {
    ModelPricing {
        input,
        output,
        cache_read: input * CACHE_READ,
        cache_write_5m: input * CACHE_WRITE_5M,
        cache_write_1h: input * CACHE_WRITE_1H,
        long_context: None,
    }
}

/// Build rates for a vendor that publishes its own cache-read price.
///
/// The multiplier helpers only fit vendors whose cache read is a tenth of input. Several
/// are nowhere near it — DeepSeek charges 2% of input for a cache hit, Z.ai 19%, xAI 25% —
/// so deriving those would be wrong by up to an order of magnitude, in the direction that
/// flatters the bill.
///
/// `cache_write` repeats the input rate. None of these vendors sells a cache write as a
/// separate product: the cache is populated as a side effect of an ordinary request, so
/// those tokens were already billed as input once. Charging a premium on top would invent
/// a line item the vendor does not have.
pub(super) fn vendor_rates(input: f64, output: f64, cache_read: f64) -> ModelPricing {
    ModelPricing {
        input,
        output,
        cache_read,
        cache_write_5m: input,
        cache_write_1h: input,
        long_context: None,
    }
}

/// Build OpenAI rates, which use the same cache multipliers as Anthropic.
///
/// OpenAI sells no 1-hour cache product, so the 1-hour column repeats the 5-minute rate
/// rather than inventing a premium — a log that somehow reports a 1-hour write against an
/// OpenAI model is then billed as an ordinary write instead of being overcharged.
pub(super) fn openai_rates(
    input: f64,
    output: f64,
    long: Option<(u64, f64, f64)>,
) -> ModelPricing {
    ModelPricing {
        input,
        output,
        cache_read: input * CACHE_READ,
        cache_write_5m: input * CACHE_WRITE_5M,
        cache_write_1h: input * CACHE_WRITE_5M,
        long_context: long.map(|(above, input, output)| LongContext {
            above,
            input,
            output,
            cache_read: input * CACHE_READ,
            cache_write_5m: input * CACHE_WRITE_5M,
            cache_write_1h: input * CACHE_WRITE_5M,
        }),
    }
}

impl ModelPricing {
    /// Cost in dollars for one breakdown.
    ///
    /// `reasoning` is deliberately absent from the arithmetic: it is a subset of `output`
    /// and is already paid for there. Charging it again is the single easiest way to
    /// inflate a bill, which is why [`TokenBreakdown`] documents it as reporting-only.
    ///
    /// The tier is chosen per request, from that request's own input size. That is the
    /// unit the threshold is defined over — a session of a hundred small requests is not
    /// a long-context request however large it sums to, and billing it as one would
    /// double the rate on work that was never charged at it.
    pub fn cost(&self, tokens: &TokenBreakdown) -> f64 {
        let per_million = |count: u64, rate: f64| (count as f64 / 1_000_000.0) * rate;

        // Everything the provider counts as input for the threshold: fresh, read from
        // cache, and written to it.
        let request_input = tokens
            .input
            .saturating_add(tokens.cache_read)
            .saturating_add(tokens.cache_write());

        let (input, output, cache_read, write_5m, write_1h) = match self.long_context {
            Some(long) if request_input > long.above => (
                long.input,
                long.output,
                long.cache_read,
                long.cache_write_5m,
                long.cache_write_1h,
            ),
            _ => (
                self.input,
                self.output,
                self.cache_read,
                self.cache_write_5m,
                self.cache_write_1h,
            ),
        };

        per_million(tokens.input, input)
            + per_million(tokens.output, output)
            + per_million(tokens.cache_read, cache_read)
            + per_million(tokens.cache_write_5m, write_5m)
            + per_million(tokens.cache_write_1h, write_1h)
    }
}

/// Why a price is or is not available — the answer `--diagnose` prints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "camelCase")]
pub enum PriceSource {
    /// From the compiled-in table.
    Bundled,
    /// From a refreshed feed, which named its origin.
    Overlay { feed: String },
    /// No rate for this model. The tokens are still counted; the cost is not guessed.
    Unpriced,
}

/// A resolved price, or an explanation of its absence.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub pricing: Option<ModelPricing>,
    pub source: PriceSource,
}

impl Resolved {
    pub fn cost(&self, tokens: &TokenBreakdown) -> Option<f64> {
        self.pricing.map(|pricing| pricing.cost(tokens))
    }
}

/// The price table in effect for one run.
#[derive(Debug, Default)]
pub struct Prices {
    /// Refreshed rates, keyed by normalized model id. Empty until a refresh succeeds.
    overlay: std::collections::HashMap<String, ModelPricing>,
    /// Where `overlay` came from, for reporting.
    feed: Option<String>,
}

impl Prices {
    /// The bundled table alone — no network, no cache, no configuration.
    pub fn bundled() -> Self {
        Prices::default()
    }

    /// Add refreshed rates on top of the bundled table.
    ///
    /// The overlay wins where the two disagree: it is newer by construction, and a vendor
    /// price change reaches the feed long before it reaches a compiled-in constant.
    pub fn with_overlay(
        mut self,
        feed: impl Into<String>,
        rates: impl IntoIterator<Item = (String, ModelPricing)>,
    ) -> Self {
        self.feed = Some(feed.into());
        self.overlay
            .extend(rates.into_iter().map(|(id, rate)| (normalize(&id), rate)));
        self
    }

    /// Resolve a model id as spelled in a log, for a message sent on `date`.
    pub fn resolve(&self, raw_model: &str, date: &str) -> Resolved {
        let id = normalize(raw_model);

        if let Some(pricing) = self.overlay.get(&id) {
            return Resolved {
                pricing: Some(*pricing),
                source: PriceSource::Overlay {
                    feed: self.feed.clone().unwrap_or_else(|| "overlay".into()),
                },
            };
        }

        match table::lookup(&id, date) {
            Some(pricing) => Resolved {
                pricing: Some(pricing),
                source: PriceSource::Bundled,
            },
            None => Resolved {
                pricing: None,
                source: PriceSource::Unpriced,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: u64, output: u64, cache_read: u64) -> TokenBreakdown {
        TokenBreakdown {
            input,
            output,
            cache_read,
            ..Default::default()
        }
    }

    #[test]
    fn a_million_of_each_bucket_costs_the_listed_rate() {
        let opus = anthropic_rates(5.0, 25.0);
        let cost = opus.cost(&tokens(1_000_000, 1_000_000, 1_000_000));
        // 5 + 25 + 0.5
        assert!((cost - 30.5).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn reasoning_tokens_are_not_billed_twice() {
        let opus = anthropic_rates(5.0, 25.0);
        let mut with_reasoning = tokens(0, 1_000_000, 0);
        with_reasoning.reasoning = 900_000;
        assert!((opus.cost(&with_reasoning) - 25.0).abs() < 1e-9);
    }

    /// A cache-heavy turn is the common case, and the one where getting the rates wrong
    /// is most expensive. These are the real counts from a local Codex turn.
    #[test]
    fn a_cache_heavy_turn_is_dominated_by_the_cheap_bucket() {
        let opus = anthropic_rates(5.0, 25.0);
        let real = crate::tokens::from_inclusive_input(71_375, 67_456, 569, 285);

        let cost = opus.cost(&real);
        // 3919 fresh input + 67456 cache read + 569 output, per million.
        let expected = 3_919.0 / 1e6 * 5.0 + 67_456.0 / 1e6 * 0.5 + 569.0 / 1e6 * 25.0;
        assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");

        // Billing the reported input figure at the fresh rate instead would be ~18x.
        let naive = 71_375.0 / 1e6 * 5.0;
        assert!(naive > cost * 4.0, "the trap is worth guarding");
    }

    #[test]
    fn the_one_hour_cache_write_costs_more_than_the_five_minute_one() {
        let opus = anthropic_rates(5.0, 25.0);
        let short = TokenBreakdown { cache_write_5m: 545_900, ..Default::default() };
        let long = TokenBreakdown { cache_write_1h: 545_900, ..Default::default() };
        assert!(opus.cost(&long) > opus.cost(&short) * 1.5);
    }

    #[test]
    fn bundled_prices_resolve_with_no_network() {
        let prices = Prices::bundled();
        let resolved = prices.resolve("claude-opus-5", "2026-08-02");
        assert_eq!(resolved.source, PriceSource::Bundled);
        assert!(resolved.pricing.is_some());
    }

    #[test]
    fn an_overlay_supersedes_the_bundled_table() {
        let repriced = anthropic_rates(4.0, 20.0);
        let prices = Prices::bundled()
            .with_overlay("models.dev", [("claude-opus-5".to_string(), repriced)]);

        let resolved = prices.resolve("claude-opus-5", "2026-08-02");
        assert_eq!(resolved.pricing, Some(repriced));
        assert_eq!(
            resolved.source,
            PriceSource::Overlay { feed: "models.dev".into() }
        );
    }

    /// The overlay is the only way a non-Anthropic model gets priced, since the bundled
    /// table documents only rates this repository can cite.
    #[test]
    fn an_overlay_can_price_a_model_the_bundle_does_not_know() {
        let prices = Prices::bundled();
        assert_eq!(
            prices.resolve("unlisted-model-1", "2026-08-02").source,
            PriceSource::Unpriced
        );

        let priced = Prices::bundled().with_overlay(
            "litellm",
            [("unlisted-model-1".to_string(), anthropic_rates(1.0, 2.0))],
        );
        assert!(priced.resolve("unlisted-model-1", "2026-08-02").pricing.is_some());
    }

    #[test]
    fn an_unpriced_model_yields_no_cost_rather_than_zero() {
        let prices = Prices::bundled();
        let resolved = prices.resolve("unlisted-model-1", "2026-08-02");
        assert_eq!(resolved.cost(&tokens(1_000, 1_000, 0)), None);
    }
}
