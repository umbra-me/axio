//! What a provider actually bills for.
//!
//! Every source normalizes into [`TokenBreakdown`], and the normalization is where the
//! money is. Three rules, each learned from a real log rather than from a schema:
//!
//! 1. **`input` is cache-exclusive.** Codex reports `input_tokens` *inclusive* of
//!    `cached_input_tokens` — one observed turn was 71,375 input of which 67,456 was
//!    cache. Billing the reported figure at the fresh-input rate overcharges by 18x.
//!    Anthropic reports the two separately and needs no adjustment. Both arrive here
//!    meaning the same thing: tokens charged at the full input rate.
//! 2. **`reasoning` is a subset of `output`, never an addend.** Codex's own arithmetic
//!    settles it: `total_tokens == input_tokens + output_tokens`, with
//!    `reasoning_output_tokens` inside the output figure. It is carried for reporting
//!    and deliberately absent from every cost calculation.
//! 3. **Cache writes split by lifetime.** A 5-minute write and a 1-hour write are
//!    different products at different prices — 1.25x and 2x the input rate. One local
//!    session wrote 545,900 tokens of 1-hour cache in a single message; charging that at
//!    the 5-minute rate understates it by 60%.

use serde::{Deserialize, Serialize};

/// Tokens for one message, in the buckets a bill is actually computed from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenBreakdown {
    /// Charged at the full input rate. Never includes cached reads.
    pub input: u64,
    /// Charged at the output rate. Includes `reasoning`.
    pub output: u64,
    /// Cache hits, charged at the (much lower) cache-read rate.
    pub cache_read: u64,
    /// Cache written with the default ~5 minute lifetime.
    pub cache_write_5m: u64,
    /// Cache written with the 1 hour lifetime, which costs more.
    pub cache_write_1h: u64,
    /// Reporting only. Already counted inside `output`; adding it to a cost is a bug.
    pub reasoning: u64,
}

impl TokenBreakdown {
    /// Every token the provider moved, for "how much did I use" questions.
    ///
    /// `reasoning` is excluded because it is already inside `output`. Saturating because a
    /// corrupt log should distort one row, not panic the whole scan.
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write_5m)
            .saturating_add(self.cache_write_1h)
    }

    pub fn cache_write(&self) -> u64 {
        self.cache_write_5m.saturating_add(self.cache_write_1h)
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Field-wise addition, for rolling many messages into one total.
    pub fn add(&mut self, other: &TokenBreakdown) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write_5m = self.cache_write_5m.saturating_add(other.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.saturating_add(other.cache_write_1h);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
    }

    /// Field-wise maximum, for collapsing repeated reports of the *same* message.
    ///
    /// This is the rule that keeps the numbers honest. A streaming API writes one message
    /// several times as it completes, and each write is the running total for that
    /// message rather than a delta — the early ones carry the input counts, the last
    /// carries the finished output. Adding them bills the input once per chunk.
    ///
    /// The maximum is correct for a cumulative report and harmless for a single one, so
    /// it needs no flag saying which kind arrived. `axio-core`'s own
    /// `Usage::merge_cumulative` reaches the same rule against live provider streams,
    /// which is the strongest evidence available that it is the real invariant.
    pub fn merge_cumulative(&mut self, other: &TokenBreakdown) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.cache_write_5m = self.cache_write_5m.max(other.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.max(other.cache_write_1h);
        self.reasoning = self.reasoning.max(other.reasoning);
    }
}

/// Build a breakdown from a source that reports input *inclusive* of cached reads.
///
/// Codex and everything speaking the Responses dialect report this way. Subtracting
/// rather than trusting the two fields independently also survives the case where a log
/// reports a cache figure larger than the input it came from, which a truncated or
/// mid-write line can do: the saturating subtraction floors at zero instead of wrapping
/// to eighteen quintillion.
pub fn from_inclusive_input(
    input_including_cache: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
) -> TokenBreakdown {
    TokenBreakdown {
        input: input_including_cache.saturating_sub(cached),
        output,
        cache_read: cached,
        cache_write_5m: 0,
        cache_write_1h: 0,
        reasoning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact figures from a local Codex turn. Trusting `input_tokens` as billable
    /// would charge for 71,375 fresh tokens instead of 3,919.
    #[test]
    fn codex_input_is_made_cache_exclusive() {
        let tokens = from_inclusive_input(71_375, 67_456, 569, 285);
        assert_eq!(tokens.input, 3_919);
        assert_eq!(tokens.cache_read, 67_456);
        assert_eq!(tokens.output, 569);
    }

    /// `total_tokens` in that same log is 71,944 — input plus output, with reasoning
    /// already inside output. The breakdown must reproduce it exactly.
    #[test]
    fn total_matches_the_providers_own_arithmetic() {
        let tokens = from_inclusive_input(71_375, 67_456, 569, 285);
        assert_eq!(tokens.total(), 71_944);
    }

    /// A cache figure exceeding its input is malformed, not a reason to wrap.
    #[test]
    fn a_cache_count_larger_than_its_input_floors_at_zero() {
        let tokens = from_inclusive_input(100, 900, 10, 0);
        assert_eq!(tokens.input, 0);
    }

    #[test]
    fn merging_a_streamed_message_keeps_the_completed_counts() {
        // The same message reported twice: input first, output once it finished.
        let mut first = TokenBreakdown {
            input: 1_006,
            cache_read: 20_992,
            ..Default::default()
        };
        let second = TokenBreakdown {
            input: 1_006,
            cache_read: 20_992,
            output: 11,
            ..Default::default()
        };
        first.merge_cumulative(&second);

        assert_eq!(first.input, 1_006, "input billed once, not twice");
        assert_eq!(first.output, 11);
        assert_eq!(first.cache_read, 20_992);
    }

    #[test]
    fn adding_rolls_distinct_messages_together() {
        let mut total = TokenBreakdown {
            input: 10,
            output: 5,
            ..Default::default()
        };
        total.add(&TokenBreakdown {
            input: 3,
            output: 2,
            cache_write_1h: 7,
            ..Default::default()
        });
        assert_eq!(total.input, 13);
        assert_eq!(total.output, 7);
        assert_eq!(total.cache_write_1h, 7);
        assert_eq!(total.cache_write(), 7);
    }

    #[test]
    fn reasoning_never_inflates_a_total() {
        let tokens = TokenBreakdown {
            input: 100,
            output: 50,
            reasoning: 40,
            ..Default::default()
        };
        assert_eq!(tokens.total(), 150, "reasoning is inside output already");
    }
}
