//! The bundled price table.
//!
//! Rates are US dollars per **million** tokens, which is how every vendor publishes them —
//! storing per-token would mean six leading zeros on every line and a table nobody can
//! proofread against the pricing page.
//!
//! Only models whose prices are documented appear here. A model that is absent is
//! reported *unpriced* rather than priced at zero: a zero row and a genuinely free model
//! are indistinguishable once summed, and the whole point of this crate is a number
//! someone can trust. `axio quota cost --diagnose` lists what was skipped and why.

use super::{ModelPricing, anthropic_rates};

/// One row of the table, before cache rates are derived.
pub(super) struct Row {
    pub(super) id: &'static str,
    pub(super) input: f64,
    pub(super) output: f64,
    /// Promotional rates and the last day they apply, inclusive, as `YYYY-MM-DD`.
    ///
    /// Cost is computed for sessions that already happened, so an introductory price is
    /// not a footnote — it is the correct rate for every message logged inside the window
    /// and the wrong one for every message after it.
    pub(super) promo: Option<(f64, f64, &'static str)>,
}

/// Anthropic models, from the published pricing table.
///
/// Cache rates are not listed per row because Anthropic derives all three from the input
/// price by fixed multipliers — see [`anthropic_rates`]. Writing them out would invite the
/// table and the multipliers to disagree.
pub(super) const ANTHROPIC: &[Row] = &[
    Row { id: "claude-fable-5", input: 10.0, output: 50.0, promo: None },
    Row { id: "claude-mythos-5", input: 10.0, output: 50.0, promo: None },
    Row { id: "claude-opus-5", input: 5.0, output: 25.0, promo: None },
    Row { id: "claude-opus-4-8", input: 5.0, output: 25.0, promo: None },
    Row { id: "claude-opus-4-7", input: 5.0, output: 25.0, promo: None },
    Row { id: "claude-opus-4-6", input: 5.0, output: 25.0, promo: None },
    Row { id: "claude-opus-4-5", input: 5.0, output: 25.0, promo: None },
    // Introductory pricing runs to the end of 2026-08. A session from inside that window
    // is billed at 2/10 and one from after it at 3/15; a single flat row would overstate
    // every Sonnet 5 session logged this month by half.
    Row {
        id: "claude-sonnet-5",
        input: 3.0,
        output: 15.0,
        promo: Some((2.0, 10.0, "2026-08-31")),
    },
    Row { id: "claude-sonnet-4-6", input: 3.0, output: 15.0, promo: None },
    Row { id: "claude-sonnet-4-5", input: 3.0, output: 15.0, promo: None },
    Row { id: "claude-haiku-4-5", input: 1.0, output: 5.0, promo: None },
];

/// OpenAI models, from `developers.openai.com/api/docs/pricing`.
///
/// `(id, input, output, long-context tier)`. The tier is `(threshold, input, output)` and
/// applies per request: past 272K input tokens the rate roughly doubles. Cache rates are
/// derived by the same multipliers as Anthropic — the published cached-input column is
/// exactly a tenth of input on every row, which is how [`super::openai_rates`] computes it.
///
/// Two aggregator sites quoted Terra at $2.50/$15 and Luna at $1.00/$6. The vendor's own
/// page says $2/$12 and $0.20/$1.20. These are the vendor's numbers.
pub(super) const OPENAI: &[(&str, f64, f64, Option<(u64, f64, f64)>)] = &[
    ("gpt-5.6-sol", 5.00, 30.00, Some((272_000, 10.00, 45.00))),
    ("gpt-5.6-terra", 2.00, 12.00, Some((272_000, 4.00, 18.00))),
    ("gpt-5.6-luna", 0.20, 1.20, Some((272_000, 0.40, 1.80))),
    ("gpt-5.5", 5.00, 30.00, Some((272_000, 10.00, 45.00))),
    ("gpt-5.5-pro", 30.00, 180.00, Some((272_000, 60.00, 270.00))),
    ("gpt-5.4", 2.50, 15.00, None),
    ("gpt-5.4-mini", 0.75, 4.50, None),
    ("gpt-5.4-nano", 0.20, 1.25, None),
    ("gpt-5.3-codex", 1.75, 14.00, None),
];

/// Everyone else, as `(id, input, output, cache read)`.
///
/// Cache reads are listed rather than derived because these vendors are nowhere near the
/// tenth-of-input that Anthropic and OpenAI share: DeepSeek charges 2% of input for a hit,
/// Z.ai 19%, xAI 25%. A derived column would be wrong by up to an order of magnitude.
///
/// Two caveats worth knowing when reading a total that includes these:
///
/// * **`glm-5.2` is quoted per provider.** Z.ai's own rate is 1.40/4.40; routing the same
///   model through a gateway has been quoted as low as 0.28/0.87. The vendor's price is
///   used, which cannot understate the bill. Z.ai also applies a peak-hours surcharge this
///   table does not model.
/// * **`gpt-5.3-codex-spark` is a research preview** whose credit rates OpenAI describes
///   as not final. It is priced at its published API rate, which is the same as
///   `gpt-5.3-codex`.
pub(super) const VENDORS: &[(&str, f64, f64, f64)] = &[
    ("deepseek-v4-flash", 0.14, 0.28, 0.0028),
    ("glm-5.2", 1.40, 4.40, 0.26),
    ("grok-4.5", 2.00, 6.00, 0.50),
    // The Grok CLI logs its build variants under their own names. Both bill as grok-4.5;
    // where the transcript carries the vendor's own `costUsdTicks` that figure wins over
    // this row anyway.
    ("grok-4.5-build", 2.00, 6.00, 0.50),
    ("grok-4.5-build-free", 2.00, 6.00, 0.50),
    ("gpt-5.3-codex-spark", 1.75, 14.00, 0.175),
];

/// Resolve a model to its rates as of `date` (`YYYY-MM-DD`).
///
/// The date is the message's own timestamp, not today's — pricing a January session at
/// August's rates is the same class of error as ignoring a promotional window.
pub(super) fn lookup(id: &str, date: &str) -> Option<ModelPricing> {
    anthropic(id, date).or_else(|| openai(id)).or_else(|| vendor(id))
}

fn vendor(id: &str) -> Option<ModelPricing> {
    VENDORS
        .iter()
        .find(|(row, ..)| *row == id)
        .map(|&(_, input, output, cache_read)| super::vendor_rates(input, output, cache_read))
}

fn openai(id: &str) -> Option<ModelPricing> {
    OPENAI
        .iter()
        .find(|(row, ..)| *row == id)
        .map(|&(_, input, output, long)| super::openai_rates(input, output, long))
}

fn anthropic(id: &str, date: &str) -> Option<ModelPricing> {
    let row = ANTHROPIC.iter().find(|row| row.id == id)?;
    let (input, output) = match row.promo {
        // Lexicographic comparison is correct for zero-padded ISO dates and avoids
        // pulling a date parser into a lookup that runs once per message.
        Some((input, output, until)) if date <= until => (input, output),
        _ => (row.input, row.output),
    };
    Some(anthropic_rates(input, output))
}

/// Normalize a logged model id to a table key.
///
/// Logs carry the exact string the client sent, which is not always the alias the price
/// table is keyed by. Three shapes show up in real transcripts:
///
/// * a dated snapshot — `claude-haiku-4-5-20251001` for `claude-haiku-4-5`
/// * a platform prefix — `anthropic.claude-opus-5` on Bedrock
/// * a vendor prefix — `anthropic/claude-fable-5`, added by routers and gateways
/// * a routing suffix — `claude-opus-5[1m]`, `claude-opus-4-8-fast`
///
/// All four name the same billable model. Stripping them here keeps the table one row
/// per model instead of one row per spelling.
pub fn normalize(raw: &str) -> String {
    let mut id = raw.trim().to_ascii_lowercase();

    // Bedrock-style platform prefixes. Vertex and the first-party API use the bare id.
    // Longest first: stripping `anthropic.` from `us.anthropic.x` would leave `us.` on.
    for prefix in ["us.anthropic.", "eu.anthropic.", "anthropic."] {
        if let Some(rest) = id.strip_prefix(prefix) {
            id = rest.to_string();
            break;
        }
    }

    // A `vendor/model` prefix. Real Codex logs on this machine carry
    // `anthropic/claude-fable-5` — a model the table does price, and which went unpriced
    // until this line existed. Keeping only the last segment also resolves a doubly
    // qualified `openrouter/anthropic/claude-opus-5`.
    if let Some((_, model)) = id.rsplit_once('/') {
        id = model.to_string();
    }

    // A bracketed routing hint (`[1m]`) is deployment metadata, never part of the id.
    if let Some(open) = id.find('[') {
        id.truncate(open);
    }

    for suffix in ["-fast", "-latest"] {
        if let Some(rest) = id.strip_suffix(suffix) {
            id = rest.to_string();
        }
    }

    // A trailing 8-digit date is a snapshot of the aliased model and bills identically.
    // Checked digit-by-digit rather than by length alone so a model whose real name ends
    // in eight characters is not silently truncated.
    if let Some((head, tail)) = id.rsplit_once('-')
        && tail.len() == 8
        && tail.bytes().all(|byte| byte.is_ascii_digit())
    {
        id = head.to_string();
    }

    id.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dated_snapshots_resolve_to_their_alias() {
        // The exact string in this machine's Claude Code logs.
        assert_eq!(normalize("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
        assert!(anthropic("claude-haiku-4-5", "2026-08-02").is_some());
    }

    #[test]
    fn a_model_name_ending_in_eight_characters_survives() {
        assert_eq!(normalize("some-model-abcdefgh"), "some-model-abcdefgh");
    }

    #[test]
    fn platform_prefixes_and_routing_suffixes_are_stripped() {
        assert_eq!(normalize("anthropic.claude-opus-5"), "claude-opus-5");
        assert_eq!(normalize("us.anthropic.claude-opus-5"), "claude-opus-5");
        assert_eq!(normalize("claude-opus-5[1m]"), "claude-opus-5");
        assert_eq!(normalize("claude-opus-4-8-fast"), "claude-opus-4-8");
    }

    /// The exact string a real Codex session on this machine logged. It went unpriced
    /// until the vendor prefix was stripped, despite naming a model the table knows.
    #[test]
    fn a_vendor_prefixed_model_resolves() {
        assert_eq!(normalize("anthropic/claude-fable-5"), "claude-fable-5");
        assert!(anthropic("claude-fable-5", "2026-08-02").is_some());
        assert_eq!(
            normalize("openrouter/anthropic/claude-opus-5"),
            "claude-opus-5"
        );
    }

    #[test]
    fn cache_rates_derive_from_the_input_price() {
        let opus = anthropic("claude-opus-5", "2026-08-02").expect("opus is priced");
        assert_eq!(opus.input, 5.0);
        assert_eq!(opus.cache_read, 0.5, "one tenth of input");
        assert_eq!(opus.cache_write_5m, 6.25, "1.25x input");
        assert_eq!(opus.cache_write_1h, 10.0, "2x input");
    }

    /// The rates axio-core hardcodes for Anthropic, reached independently. If these ever
    /// disagree, one of the two is wrong about what a turn costs.
    #[test]
    fn the_opus_row_matches_axio_cores_own_numbers() {
        let opus = anthropic("claude-opus-5", "2026-08-02").expect("opus is priced");
        assert_eq!(
            (opus.input, opus.output, opus.cache_read, opus.cache_write_5m),
            (5.0, 25.0, 0.5, 6.25)
        );
    }

    #[test]
    fn introductory_pricing_applies_inside_its_window_and_not_after() {
        let during = anthropic("claude-sonnet-5", "2026-08-02").expect("priced");
        assert_eq!((during.input, during.output), (2.0, 10.0));

        let last_day = anthropic("claude-sonnet-5", "2026-08-31").expect("priced");
        assert_eq!(last_day.input, 2.0, "the window is inclusive");

        let after = anthropic("claude-sonnet-5", "2026-09-01").expect("priced");
        assert_eq!((after.input, after.output), (3.0, 15.0));
    }

    /// The promotional input rate must drag the cache rates down with it — they are
    /// multiples of whatever input actually costs that day, not of the list price.
    #[test]
    fn promotional_rates_carry_through_to_cache() {
        let during = anthropic("claude-sonnet-5", "2026-08-02").expect("priced");
        assert_eq!(during.cache_read, 0.2);
        assert_eq!(during.cache_write_5m, 2.5);
    }

    #[test]
    fn an_unknown_model_is_unpriced_rather_than_free() {
        assert!(anthropic("gpt-5.6-terra", "2026-08-02").is_none());
        assert!(anthropic("glm-5.2", "2026-08-02").is_none());
    }
}

#[cfg(test)]
mod openai_tests {
    use super::*;
    use crate::tokens::TokenBreakdown;

    /// The vendor's own figures, against two aggregators that disagreed with them.
    #[test]
    fn openai_rows_match_the_vendors_published_table() {
        for (id, input, cached, output) in [
            ("gpt-5.6-sol", 5.00, 0.50, 30.00),
            ("gpt-5.6-terra", 2.00, 0.20, 12.00),
            ("gpt-5.6-luna", 0.20, 0.02, 1.20),
            ("gpt-5.5", 5.00, 0.50, 30.00),
            ("gpt-5.3-codex", 1.75, 0.175, 14.00),
        ] {
            let rates = lookup(id, "2026-08-02").unwrap_or_else(|| panic!("{id} is priced"));
            assert_eq!(rates.input, input, "{id} input");
            assert_eq!(rates.output, output, "{id} output");
            assert!((rates.cache_read - cached).abs() < 1e-9, "{id} cached input");
        }
    }

    /// 1.83% of the Codex requests on this machine exceed the threshold. Ignoring the
    /// tier understates those by half.
    #[test]
    fn a_request_over_272k_is_billed_at_the_long_rate() {
        let sol = lookup("gpt-5.6-sol", "2026-08-02").expect("priced");

        let short = TokenBreakdown { input: 100_000, output: 1_000, ..Default::default() };
        let long = TokenBreakdown { input: 300_000, output: 1_000, ..Default::default() };

        // 100k at $5 + 1k at $30
        assert!((sol.cost(&short) - (0.5 + 0.03)).abs() < 1e-9, "{}", sol.cost(&short));
        // 300k at $10 + 1k at $45
        assert!((sol.cost(&long) - (3.0 + 0.045)).abs() < 1e-9, "{}", sol.cost(&long));
    }

    /// The threshold counts everything the provider treats as input, so a request that is
    /// mostly cache reads still crosses it.
    #[test]
    fn cached_reads_count_toward_the_threshold() {
        let sol = lookup("gpt-5.6-sol", "2026-08-02").expect("priced");
        let cached = TokenBreakdown { input: 1_000, cache_read: 300_000, ..Default::default() };
        // Long tier: 1k at $10 + 300k at $1.00
        assert!((sol.cost(&cached) - (0.01 + 0.3)).abs() < 1e-9, "{}", sol.cost(&cached));
    }

    /// Anthropic's current models do not tier, so a huge Opus request stays at one rate.
    #[test]
    fn anthropic_models_have_no_long_context_tier() {
        let opus = lookup("claude-opus-5", "2026-08-02").expect("priced");
        assert!(opus.long_context.is_none());
    }

    /// Absent from the vendor's own rate card, but published elsewhere at the same rate
    /// as `gpt-5.3-codex`. Priced at that, with the preview caveat recorded on VENDORS.
    #[test]
    fn the_codex_spark_preview_is_priced_like_its_sibling() {
        let spark = lookup("gpt-5.3-codex-spark", "2026-08-03").expect("priced");
        let codex = lookup("gpt-5.3-codex", "2026-08-03").expect("priced");
        assert_eq!((spark.input, spark.output), (codex.input, codex.output));
    }
}

#[cfg(test)]
mod vendor_tests {
    use super::*;

    /// The published cache-read rates, none of which is a tenth of input. Deriving them
    /// would have been wrong by up to 5x in the direction that flatters the bill.
    #[test]
    fn vendor_cache_rates_are_listed_not_derived() {
        for (id, input, cached) in [
            ("deepseek-v4-flash", 0.14, 0.0028),
            ("glm-5.2", 1.40, 0.26),
            ("grok-4.5", 2.00, 0.50),
        ] {
            let rates = lookup(id, "2026-08-03").unwrap_or_else(|| panic!("{id} is priced"));
            assert_eq!(rates.input, input, "{id} input");
            assert_eq!(rates.cache_read, cached, "{id} cache read");
            assert_ne!(
                rates.cache_read,
                input * 0.10,
                "{id} would have been wrong if derived"
            );
        }
    }

    /// Every model seen in the local transcripts must now resolve. This is the list the
    /// scan reported as unpriced before these rows existed.
    #[test]
    fn every_locally_observed_model_is_priced() {
        for id in [
            "claude-opus-5",
            "claude-fable-5",
            "claude-opus-4-8",
            "claude-sonnet-5",
            "claude-haiku-4-5-20251001",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.3-codex-spark",
            "deepseek-v4-flash",
            "glm-5.2",
            "grok-4.5-build",
            "grok-4.5-build-free",
            "anthropic/claude-fable-5",
        ] {
            assert!(
                lookup(&normalize(id), "2026-08-03").is_some(),
                "{id} is still unpriced"
            );
        }
    }

    /// A cache write is not a separate product for these vendors — the cache is filled by
    /// an ordinary request whose tokens were billed as input once already.
    #[test]
    fn a_vendor_cache_write_is_charged_as_plain_input() {
        let deepseek = lookup("deepseek-v4-flash", "2026-08-03").expect("priced");
        assert_eq!(deepseek.cache_write_5m, deepseek.input);
        assert_eq!(deepseek.cache_write_1h, deepseek.input);
    }

    #[test]
    fn an_unknown_model_is_still_unpriced_rather_than_free() {
        assert!(lookup("some-model-nobody-published", "2026-08-03").is_none());
    }
}
