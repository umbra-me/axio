//! Summing messages into a number you are allowed to show someone.
//!
//! The hard part of this module is not addition. It is that a sum over partially-priced
//! data is not a cost, and every honest surface has to be stopped from printing it as one.
//!
//! The failure this exists to prevent was real. A scan of 77,525 Codex messages found one
//! whose model the price table knew; the other 77,524 ran on models it did not. Summing
//! what could be priced gave `$0.30`, which is arithmetically correct and, printed beside
//! the word *Codex*, a lie — it reads as the cost of the whole agent rather than of
//! 0.0003% of it.
//!
//! So [`Totals`] does not expose an `f64`. It exposes [`Cost`], which cannot be formatted
//! without confronting how much of the underlying volume it actually covers.

use std::collections::BTreeSet;

use crate::message::CostMessage;
use crate::pricing::Prices;
use crate::sources::date_of;
use crate::tokens::TokenBreakdown;

/// A cost, and how much of the data it speaks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Cost {
    /// Every message had a price. The number means what it says.
    Complete(f64),
    /// Some models had no price. `covered` is the share of tokens the figure accounts
    /// for, in `0.0..1.0` — a caller that prints `dollars` without it is misreporting.
    Partial { dollars: f64, covered: f64 },
    /// Nothing could be priced. There is no number to show, only tokens to count.
    Unknown,
}

impl Cost {
    /// The figure only when it stands for the whole total.
    ///
    /// `None` for a partial or unknown cost, so summing agents cannot silently produce a
    /// grand total that is missing most of its input.
    pub fn complete(&self) -> Option<f64> {
        match self {
            Cost::Complete(dollars) => Some(*dollars),
            _ => None,
        }
    }

    /// The figure with its coverage, for a caller prepared to render both.
    pub fn partial(&self) -> Option<(f64, f64)> {
        match self {
            Cost::Complete(dollars) => Some((*dollars, 1.0)),
            Cost::Partial { dollars, covered } => Some((*dollars, *covered)),
            Cost::Unknown => None,
        }
    }
}

/// Below this share of priced tokens, a dollar figure is noise dressed as a number.
///
/// One priced message in 77,525 is 0.0003% — comfortably under. The threshold is not a
/// rounding convenience: it is the line past which showing the number misinforms more
/// than showing nothing.
const NEGLIGIBLE_COVERAGE: f64 = 0.01;

/// What a group of messages used and cost.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub messages: usize,
    pub tokens: TokenBreakdown,
    /// Cost of the messages that resolved to a price.
    priced_dollars: f64,
    /// Tokens belonging to messages that did not.
    pub unpriced_tokens: u64,
    pub unpriced_messages: usize,
    /// Models seen with no price, so a report can say which feed would fix it.
    pub unpriced_models: BTreeSet<String>,
    /// Messages costed by the agent's own figure rather than by the table.
    pub reported_messages: usize,
}

impl Totals {
    /// Fold one message in, pricing it as of its own date.
    ///
    /// The message's date rather than today's: an introductory rate that ended last month
    /// is still the right rate for a session that ran while it was live.
    pub fn add(&mut self, message: &CostMessage, prices: &Prices) {
        self.messages += 1;
        self.tokens.add(&message.tokens);

        // The agent's own figure wins when it exists. It is the only number here that
        // was computed by the party actually doing the charging.
        if let Some(dollars) = message.reported_cost {
            self.priced_dollars += dollars;
            self.reported_messages += 1;
            return;
        }

        let date = message.timestamp.date().to_string();
        match prices.resolve(&message.model, date_of(&date)).cost(&message.tokens) {
            Some(dollars) => self.priced_dollars += dollars,
            None => {
                self.unpriced_tokens += message.tokens.total();
                self.unpriced_messages += 1;
                self.unpriced_models.insert(message.model.clone());
            }
        }
    }

    pub fn extend<'a>(
        &mut self,
        messages: impl IntoIterator<Item = &'a CostMessage>,
        prices: &Prices,
    ) {
        for message in messages {
            self.add(message, prices);
        }
    }

    /// Share of tokens that carry a price, in `0.0..=1.0`.
    ///
    /// Weighted by tokens rather than by message count, because that is what the money
    /// follows: a thousand priced one-line replies do not make up for one unpriced
    /// million-token turn.
    pub fn coverage(&self) -> f64 {
        let total = self.tokens.total();
        if total == 0 {
            return 1.0;
        }
        1.0 - (self.unpriced_tokens as f64 / total as f64)
    }

    /// The cost, classified by how much of the volume it accounts for.
    pub fn cost(&self) -> Cost {
        if self.unpriced_messages == 0 {
            return Cost::Complete(self.priced_dollars);
        }
        let covered = self.coverage();
        if covered < NEGLIGIBLE_COVERAGE {
            return Cost::Unknown;
        }
        Cost::Partial { dollars: self.priced_dollars, covered }
    }

    /// Merge another group's totals in.
    pub fn merge(&mut self, other: &Totals) {
        self.messages += other.messages;
        self.tokens.add(&other.tokens);
        self.priced_dollars += other.priced_dollars;
        self.unpriced_tokens += other.unpriced_tokens;
        self.unpriced_messages += other.unpriced_messages;
        self.reported_messages += other.reported_messages;
        self.unpriced_models.extend(other.unpriced_models.iter().cloned());
    }
}

/// Render a cost for a table, never claiming more than it knows.
///
/// Two details that look like fussiness and are not. The percentage is **floored**, so a
/// total that is 99.96% priced reads `99.9%` rather than rounding up to a `100%` that
/// would be indistinguishable from complete. And the word *only* appears just below 90%,
/// because attaching it to `99.9%` cries wolf and trains the reader to skip the caveat on
/// the row where it matters.
pub fn render(cost: &Cost) -> String {
    match cost {
        Cost::Complete(dollars) => format!("${dollars:.2}"),
        // The coverage rides along with the figure rather than in a footnote, because a
        // footnote is exactly what gets dropped when someone copies a row into a summary.
        Cost::Partial { dollars, covered } => {
            let percent = (covered * 1000.0).floor() / 10.0;
            let qualifier = if *covered < 0.9 { "only " } else { "" };
            format!("${dollars:.2} ({qualifier}{percent:.1}% priced)")
        }
        Cost::Unknown => "unpriced".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ClientId;
    use time::macros::datetime;

    fn message(model: &str, input: u64) -> CostMessage {
        CostMessage {
            client: ClientId::new("codex"),
            model: model.into(),
            session_id: "s".into(),
            workspace: None,
            timestamp: datetime!(2026-08-02 10:00 UTC),
            tokens: TokenBreakdown { input, ..Default::default() },
            dedup_key: None,
            turn_start: false,
            reported_cost: None,
        }
    }

    #[test]
    fn a_fully_priced_group_reports_a_plain_number() {
        let mut totals = Totals::default();
        totals.add(&message("claude-opus-5", 1_000_000), &Prices::bundled());
        assert_eq!(totals.cost(), Cost::Complete(5.0));
        assert_eq!(render(&totals.cost()), "$5.00");
    }

    /// The exact shape that produced the bad `$0.30`: one priced message among tens of
    /// thousands. It must refuse to show a number at all.
    #[test]
    fn one_priced_message_in_a_sea_of_unpriced_ones_is_not_a_cost() {
        let mut totals = Totals::default();
        totals.add(&message("claude-fable-5", 30_410), &Prices::bundled());
        for _ in 0..100 {
            totals.add(&message("unlisted-model-1", 10_000_000), &Prices::bundled());
        }

        assert_eq!(totals.cost(), Cost::Unknown);
        assert_eq!(render(&totals.cost()), "unpriced");
        assert!(totals.coverage() < 0.001);
        assert_eq!(totals.cost().complete(), None, "cannot leak into a grand total");
    }

    #[test]
    fn a_materially_priced_group_shows_its_coverage() {
        let mut totals = Totals::default();
        totals.add(&message("claude-opus-5", 700_000), &Prices::bundled());
        totals.add(&message("unlisted-model-1", 300_000), &Prices::bundled());

        let cost = totals.cost();
        let (dollars, covered) = cost.partial().expect("partial");
        assert!((dollars - 3.5).abs() < 1e-9);
        assert!((covered - 0.7).abs() < 1e-9);
        assert_eq!(render(&cost), "$3.50 (only 70.0% priced)");
    }

    /// Coverage follows tokens, not message counts — the money is in the volume.
    #[test]
    fn coverage_is_weighted_by_tokens_not_by_message_count() {
        let mut totals = Totals::default();
        for _ in 0..999 {
            totals.add(&message("claude-opus-5", 1), &Prices::bundled());
        }
        totals.add(&message("unlisted-model-1", 1_000_000), &Prices::bundled());
        assert!(totals.coverage() < 0.01, "{}", totals.coverage());
        assert_eq!(totals.cost(), Cost::Unknown);
    }

    #[test]
    fn unpriced_models_are_named_so_a_report_can_say_what_is_missing() {
        let mut totals = Totals::default();
        totals.add(&message("unlisted-model-2", 10), &Prices::bundled());
        totals.add(&message("unlisted-model-1", 10), &Prices::bundled());
        totals.add(&message("unlisted-model-2", 10), &Prices::bundled());
        assert_eq!(
            totals.unpriced_models.iter().cloned().collect::<Vec<_>>(),
            vec!["unlisted-model-1".to_string(), "unlisted-model-2".to_string()]
        );
    }

    #[test]
    fn merging_preserves_both_the_cost_and_the_doubt() {
        let (mut priced, mut unpriced) = (Totals::default(), Totals::default());
        priced.add(&message("claude-opus-5", 1_000_000), &Prices::bundled());
        unpriced.add(&message("unlisted-model-1", 1_000_000), &Prices::bundled());

        priced.merge(&unpriced);
        assert_eq!(priced.messages, 2);
        let (dollars, covered) = priced.cost().partial().expect("partial");
        assert!((dollars - 5.0).abs() < 1e-9);
        assert!((covered - 0.5).abs() < 1e-9);
    }

    /// 99.96% must not print as `100%`: the reader cannot tell that apart from complete,
    /// which is the whole distinction this type exists to preserve.
    #[test]
    fn a_nearly_complete_total_never_rounds_up_to_a_hundred() {
        let mut totals = Totals::default();
        totals.add(&message("claude-opus-5", 9_996), &Prices::bundled());
        totals.add(&message("unlisted-model-1", 4), &Prices::bundled());
        let rendered = render(&totals.cost());
        assert!(rendered.ends_with("(99.9% priced)"), "{rendered}");
        assert!(!rendered.contains("only"), "not a warning at 99.9%: {rendered}");
    }

    #[test]
    fn an_empty_group_is_complete_at_zero_rather_than_unknown() {
        let totals = Totals::default();
        assert_eq!(totals.cost(), Cost::Complete(0.0));
    }
}
