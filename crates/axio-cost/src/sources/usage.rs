//! Reading a usage object whoever wrote it.
//!
//! Across the agents worth reading there are three ways to spell the same four numbers,
//! and — more dangerously — two incompatible conventions about what they mean.
//!
//! | Spelling | Example keys | Convention |
//! | --- | --- | --- |
//! | Anthropic | `input_tokens`, `cache_read_input_tokens` | input **excludes** cache |
//! | OpenAI | `prompt_tokens`, `cached_tokens` | input **includes** cache |
//! | camelCase | `inputTokens`, `cacheReadTokens` | either — the agent must say |
//!
//! The spelling is usually enough to settle the convention, because an agent that speaks
//! `prompt_tokens` is relaying an OpenAI-shaped response and inherits its meaning. Where
//! it is not enough the source declares it, because guessing wrong is not a rounding
//! error: on a cache-heavy turn, treating inclusive input as exclusive overcharges by an
//! order of magnitude.

use serde::Deserialize;

use crate::tokens::{TokenBreakdown, from_inclusive_input};

/// Whether a source's input figure already contains its cached reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convention {
    /// `input` and `cache_read` are disjoint. Anthropic's shape.
    CacheExclusive,
    /// `input` contains `cache_read`. The Responses/chat-completions shape.
    CacheInclusive,
    /// Follow whichever spelling the object actually used, defaulting to exclusive.
    ///
    /// For agents that relay whatever their upstream returned and so may write either
    /// shape from one session to the next.
    BySpelling,
}

/// Every spelling of a usage object, in one shape.
///
/// Fields are collected by [`serde`] alias rather than by trying several structs in turn,
/// so an object that mixes spellings — and several do — still yields every number it
/// carries instead of only those from the first struct that happened to parse.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct AnyUsage {
    #[serde(default, alias = "inputTokens", alias = "input")]
    pub input_tokens: Option<u64>,
    #[serde(default, alias = "promptTokens")]
    pub prompt_tokens: Option<u64>,

    #[serde(default, alias = "outputTokens", alias = "output")]
    pub output_tokens: Option<u64>,
    #[serde(default, alias = "completionTokens")]
    pub completion_tokens: Option<u64>,

    #[serde(
        default,
        alias = "cache_read_input_tokens",
        alias = "cacheReadTokens",
        alias = "cachedReadTokens",
        alias = "cached_input_tokens",
        alias = "cachedInputTokens",
        alias = "cacheReadInputTokens"
    )]
    pub cached_tokens: Option<u64>,

    #[serde(
        default,
        alias = "cache_creation_input_tokens",
        alias = "cacheWriteTokens",
        alias = "cacheCreationInputTokens",
        alias = "cache_write"
    )]
    pub cache_write_tokens: Option<u64>,

    #[serde(default, alias = "reasoningTokens", alias = "reasoning_output_tokens")]
    pub reasoning_tokens: Option<u64>,

    /// Only ever used as a fallback — see [`AnyUsage::breakdown`].
    #[serde(default, alias = "totalTokens", alias = "total")]
    pub total_tokens: Option<u64>,
}

impl AnyUsage {
    /// Whether this object carries anything worth billing.
    pub fn is_empty(&self) -> bool {
        [
            self.input_tokens,
            self.prompt_tokens,
            self.output_tokens,
            self.completion_tokens,
            self.cached_tokens,
            self.cache_write_tokens,
            self.total_tokens,
        ]
        .iter()
        .all(|value| value.unwrap_or(0) == 0)
    }

    /// Normalize into the crate's buckets.
    ///
    /// A `total`-only object — three agents report nothing else — becomes all-output.
    /// That is deliberately the **expensive** bucket: output costs several times input
    /// everywhere, so an agent that will not say how its tokens split is costed at the
    /// rate that cannot understate it. Better a figure that is too high and flagged than
    /// one that is quietly too low.
    pub fn breakdown(&self, convention: Convention) -> TokenBreakdown {
        let output = self.output_tokens.or(self.completion_tokens).unwrap_or(0);
        let cached = self.cached_tokens.unwrap_or(0);
        let reasoning = self.reasoning_tokens.unwrap_or(0);
        let cache_write = self.cache_write_tokens.unwrap_or(0);

        // Which spelling supplied the input decides the convention when the source has
        // not pinned one: `prompt_tokens` is an OpenAI-shaped response and carries its
        // meaning with it.
        let (input, inclusive) = match (self.input_tokens, self.prompt_tokens) {
            (Some(input), _) => (input, convention == Convention::CacheInclusive),
            (None, Some(prompt)) => (prompt, convention != Convention::CacheExclusive),
            (None, None) => (0, false),
        };

        if input == 0
            && output == 0
            && cached == 0
            && let Some(total) = self.total_tokens.filter(|total| *total > 0)
        {
            return TokenBreakdown {
                output: total,
                ..Default::default()
            };
        }

        if inclusive {
            let mut tokens = from_inclusive_input(input, cached, output, reasoning);
            tokens.cache_write_5m = cache_write;
            return tokens;
        }

        TokenBreakdown {
            input,
            output,
            cache_read: cached,
            cache_write_5m: cache_write,
            cache_write_1h: 0,
            reasoning,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> AnyUsage {
        serde_json::from_str(json).expect("parses")
    }

    #[test]
    fn the_anthropic_spelling_keeps_input_and_cache_disjoint() {
        let usage = parse(
            r#"{"input_tokens":1006,"output_tokens":11,"cache_read_input_tokens":20992,"cache_creation_input_tokens":50}"#,
        );
        let tokens = usage.breakdown(Convention::BySpelling);
        assert_eq!(tokens.input, 1_006);
        assert_eq!(tokens.cache_read, 20_992);
        assert_eq!(tokens.cache_write_5m, 50);
        assert_eq!(tokens.total(), 22_059);
    }

    /// `prompt_tokens` means the OpenAI shape, where the cached figure is *inside* the
    /// prompt figure. Reading it as disjoint would bill 100k of fresh input instead of 10k.
    #[test]
    fn the_openai_spelling_subtracts_its_cached_reads() {
        let usage =
            parse(r#"{"prompt_tokens":100000,"completion_tokens":500,"cached_tokens":90000}"#);
        let tokens = usage.breakdown(Convention::BySpelling);
        assert_eq!(tokens.input, 10_000);
        assert_eq!(tokens.cache_read, 90_000);
        assert_eq!(tokens.output, 500);
    }

    #[test]
    fn a_source_may_override_the_spellings_default() {
        let usage = parse(r#"{"inputTokens":100000,"outputTokens":5,"cacheReadTokens":90000}"#);
        assert_eq!(usage.breakdown(Convention::CacheExclusive).input, 100_000);
        assert_eq!(usage.breakdown(Convention::CacheInclusive).input, 10_000);
    }

    /// Three agents report only a total. Costing it as output cannot understate the bill,
    /// which is the safer direction to be wrong in.
    #[test]
    fn a_total_only_object_is_costed_at_the_output_rate() {
        let usage = parse(r#"{"totalTokens":11491}"#);
        let tokens = usage.breakdown(Convention::BySpelling);
        assert_eq!(tokens.output, 11_491);
        assert_eq!(tokens.input, 0);
        assert_eq!(tokens.total(), 11_491);
    }

    /// A total alongside a real split must not override the split.
    #[test]
    fn a_total_is_ignored_when_the_parts_are_present() {
        let usage = parse(r#"{"inputTokens":100,"outputTokens":10,"totalTokens":110}"#);
        let tokens = usage.breakdown(Convention::BySpelling);
        assert_eq!(tokens.input, 100);
        assert_eq!(tokens.output, 10);
        assert_eq!(tokens.total(), 110, "not 220");
    }

    #[test]
    fn mixed_spellings_in_one_object_are_all_collected() {
        // Observed shape: an agent relaying one upstream while logging in its own style.
        let usage = parse(r#"{"input_tokens":50,"completionTokens":7,"cacheReadTokens":9}"#);
        let tokens = usage.breakdown(Convention::CacheExclusive);
        assert_eq!((tokens.input, tokens.output, tokens.cache_read), (50, 7, 9));
    }

    #[test]
    fn an_empty_or_zeroed_object_is_recognised_as_such() {
        assert!(parse("{}").is_empty());
        assert!(parse(r#"{"input_tokens":0,"output_tokens":0}"#).is_empty());
        assert!(!parse(r#"{"output_tokens":1}"#).is_empty());
    }

    #[test]
    fn reasoning_rides_along_without_inflating_the_total() {
        let usage = parse(r#"{"input_tokens":10,"output_tokens":20,"reasoning_tokens":15}"#);
        let tokens = usage.breakdown(Convention::CacheExclusive);
        assert_eq!(tokens.reasoning, 15);
        assert_eq!(tokens.total(), 30, "reasoning is inside output");
    }
}
