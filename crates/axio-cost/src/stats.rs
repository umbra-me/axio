//! The shape of a year's usage: a day-by-day series, and what it says about habit.
//!
//! Everything here is derived from message timestamps, so it costs one pass over data the
//! scan already produced. The calendar is the interesting part — a table answers *what did
//! I spend*, and a year of days answers *when do I actually work*, which is a different
//! question and not one any grouping in the table can reach.
//!
//! Days are UTC. Every timestamp in these transcripts is written in UTC and converting to
//! local time would move messages across midnight, which would silently redraw the
//! calendar and change a streak depending on where the machine is.

use std::collections::BTreeMap;

use time::Date;

use crate::message::CostMessage;
use crate::pricing::Prices;
use crate::totals::Totals;

/// One day of the calendar.
#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub date: Date,
    pub messages: usize,
    pub tokens: u64,
    /// `None` when nothing that day could be priced.
    pub cost: Option<f64>,
}

/// A year of days, plus what the pattern says.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// Every day that saw activity, oldest first. Empty days are absent rather than zero —
    /// the caller draws the grid and knows which days it is drawing.
    pub days: Vec<Day>,
    pub active_days: usize,
    /// Consecutive active days ending at the most recent activity.
    pub current_streak: usize,
    pub longest_streak: usize,
    pub sessions: usize,
    /// The model with the most tokens through it.
    pub top_model: Option<String>,
    pub totals: Totals,
    /// Tokens by hour of the day, UTC, `0..24`.
    ///
    /// The one figure here that is worth reading in local time and is not — see the module
    /// note. Reported as UTC and labelled as UTC, rather than converted silently, because
    /// a histogram that shifts when you fly somewhere is worse than one that is honest
    /// about which clock it is on.
    pub by_hour: [u64; 24],
    /// Tokens by weekday, Sunday first, matching the calendar grid above it.
    pub by_weekday: [u64; 7],
}

impl Stats {
    /// The busiest day's token count.
    pub fn busiest(&self) -> u64 {
        self.days.iter().map(|day| day.tokens).max().unwrap_or(0)
    }

    /// The three cuts that split active days into four heat-map levels.
    ///
    /// Quartiles of the active days, and not a ramp scaled against the busiest day. Both
    /// were tried against real data and only this one draws anything. Scaling against the
    /// maximum fails linearly *and* logarithmically for the same underlying reason: a
    /// year of daily token counts spans several orders of magnitude, so a linear ramp puts
    /// almost everything in the lowest bucket and a log ramp puts almost everything in the
    /// highest — 50M against a 1.8B peak is 83% of the way up a log scale.
    ///
    /// Quartiles ask a different and more useful question. Not "how does this day compare
    /// to my single biggest ever", which every day loses, but "how does this day compare
    /// to my typical day", which is what someone reading their own calendar means. The
    /// four levels are then guaranteed to be populated, which is the property that makes
    /// the grid a picture of a working pattern rather than a wall of one colour.
    ///
    /// Days with no activity are excluded before the quartiles are taken: they are already
    /// level zero, and leaving them in would drag every cut toward nothing.
    pub fn thresholds(&self) -> [u64; 3] {
        let mut tokens: Vec<u64> = self
            .days
            .iter()
            .map(|day| day.tokens)
            .filter(|tokens| *tokens > 0)
            .collect();
        if tokens.is_empty() {
            return [0; 3];
        }
        tokens.sort_unstable();
        let at = |fraction: f64| {
            let index = (tokens.len() as f64 * fraction) as usize;
            tokens[index.min(tokens.len() - 1)]
        };
        [at(0.25), at(0.50), at(0.75)]
    }

    /// Which of the five levels a day's tokens fall into.
    pub fn level(thresholds: &[u64; 3], tokens: u64) -> usize {
        if tokens == 0 {
            return 0;
        }
        1 + thresholds.iter().filter(|cut| tokens >= **cut).count()
    }
}

/// Fold messages into a calendar.
pub fn summarise<'a>(
    messages: impl IntoIterator<Item = &'a CostMessage>,
    prices: &Prices,
) -> Stats {
    let mut by_day: BTreeMap<Date, (usize, u64, Totals)> = BTreeMap::new();
    let mut by_model: BTreeMap<String, u64> = BTreeMap::new();
    let mut sessions: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut totals = Totals::default();
    let mut by_hour = [0u64; 24];
    let mut by_weekday = [0u64; 7];

    for message in messages {
        let date = message.timestamp.date();
        by_hour[message.timestamp.hour() as usize] += message.tokens.total();
        by_weekday[date.weekday().number_days_from_sunday() as usize] += message.tokens.total();
        let entry = by_day.entry(date).or_default();
        entry.0 += 1;
        entry.1 += message.tokens.total();
        entry.2.add(message, prices);

        *by_model
            .entry(crate::pricing::normalize(&message.model))
            .or_default() += message.tokens.total();
        sessions.insert(message.session_id.as_str());
        totals.add(message, prices);
    }

    let days: Vec<Day> = by_day
        .into_iter()
        .map(|(date, (messages, tokens, totals))| Day {
            date,
            messages,
            tokens,
            // A day whose models were all unpriced reports no cost rather than zero, the
            // same distinction the tables make.
            cost: totals.cost().partial().map(|(dollars, _)| dollars),
        })
        .collect();

    let dates: Vec<Date> = days.iter().map(|day| day.date).collect();

    Stats {
        active_days: days.len(),
        current_streak: current_streak(&dates),
        longest_streak: longest_streak(&dates),
        sessions: sessions.len(),
        top_model: by_model
            .into_iter()
            .max_by_key(|(_, tokens)| *tokens)
            .map(|(model, _)| model),
        days,
        totals,
        by_hour,
        by_weekday,
    }
}

/// The run of consecutive days ending at the most recent one.
///
/// Anchored on the last *active* day rather than on today, so a streak is a property of
/// the data rather than of when the report happens to be run. A caller that wants "is the
/// streak still alive" compares the last day against today itself — that is a question
/// about now, and this function deliberately does not know what now is.
fn current_streak(dates: &[Date]) -> usize {
    let Some(&last) = dates.last() else {
        return 0;
    };
    let mut streak = 1;
    let mut expected = last.previous_day();

    for &date in dates.iter().rev().skip(1) {
        match expected {
            Some(previous) if date == previous => {
                streak += 1;
                expected = date.previous_day();
            }
            _ => break,
        }
    }
    streak
}

/// The longest run of consecutive days anywhere in the series.
fn longest_streak(dates: &[Date]) -> usize {
    let (mut best, mut run) = (0, 0);
    let mut previous: Option<Date> = None;

    for &date in dates {
        run = match previous {
            Some(earlier) if earlier.next_day() == Some(date) => run + 1,
            _ => 1,
        };
        best = best.max(run);
        previous = Some(date);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ClientId;
    use crate::tokens::TokenBreakdown;
    use time::macros::{date, datetime};

    fn on(day: Date, model: &str, tokens: u64) -> CostMessage {
        CostMessage {
            client: ClientId::new("codex"),
            model: model.into(),
            session_id: format!("s-{day}"),
            workspace: None,
            timestamp: datetime!(2026-01-01 12:00 UTC).replace_date(day),
            tokens: TokenBreakdown {
                input: tokens,
                ..Default::default()
            },
            dedup_key: None,
            turn_start: false,
            reported_cost: None,
        }
    }

    #[test]
    fn days_are_folded_and_ordered_oldest_first() {
        let messages = vec![
            on(date!(2026 - 08 - 03), "claude-opus-5", 10),
            on(date!(2026 - 08 - 01), "claude-opus-5", 20),
            on(date!(2026 - 08 - 03), "claude-opus-5", 5),
        ];
        let stats = summarise(&messages, &Prices::bundled());

        assert_eq!(stats.days.len(), 2);
        assert_eq!(stats.days[0].date, date!(2026 - 08 - 01));
        assert_eq!(stats.days[1].tokens, 15, "same day folds together");
        assert_eq!(stats.active_days, 2);
    }

    #[test]
    fn a_run_of_days_is_the_current_streak() {
        let messages: Vec<_> = [1, 2, 3, 4]
            .iter()
            .map(|day| on(date!(2026 - 08 - 01).replace_day(*day).unwrap(), "m", 1))
            .collect();
        let stats = summarise(&messages, &Prices::bundled());
        assert_eq!(stats.current_streak, 4);
        assert_eq!(stats.longest_streak, 4);
    }

    /// A gap ends the current streak but not the record.
    #[test]
    fn a_gap_breaks_the_current_streak_and_the_longest_survives() {
        let days = [1, 2, 3, 4, 5, 20, 21];
        let messages: Vec<_> = days
            .iter()
            .map(|day| on(date!(2026 - 08 - 01).replace_day(*day).unwrap(), "m", 1))
            .collect();
        let stats = summarise(&messages, &Prices::bundled());

        assert_eq!(stats.current_streak, 2, "the 20th and 21st");
        assert_eq!(stats.longest_streak, 5, "the 1st through 5th");
    }

    /// A streak has to survive a month boundary, which is the case a naive day-number
    /// comparison gets wrong.
    #[test]
    fn a_streak_crosses_the_end_of_a_month() {
        let messages = vec![
            on(date!(2026 - 07 - 30), "m", 1),
            on(date!(2026 - 07 - 31), "m", 1),
            on(date!(2026 - 08 - 01), "m", 1),
        ];
        let stats = summarise(&messages, &Prices::bundled());
        assert_eq!(stats.current_streak, 3);
    }

    #[test]
    fn one_day_is_a_streak_of_one_and_no_days_is_none() {
        let single = summarise(&[on(date!(2026 - 08 - 03), "m", 1)], &Prices::bundled());
        assert_eq!(single.current_streak, 1);
        assert_eq!(single.longest_streak, 1);

        let empty = summarise(std::iter::empty(), &Prices::bundled());
        assert_eq!(empty.current_streak, 0);
        assert_eq!(empty.longest_streak, 0);
        assert_eq!(empty.busiest(), 0);
    }

    #[test]
    fn the_top_model_is_the_one_with_the_most_tokens() {
        let messages = vec![
            on(date!(2026 - 08 - 01), "claude-opus-5", 10),
            on(date!(2026 - 08 - 01), "gpt-5.6-sol", 100),
            on(date!(2026 - 08 - 02), "gpt-5.6-sol", 1),
        ];
        let stats = summarise(&messages, &Prices::bundled());
        assert_eq!(stats.top_model.as_deref(), Some("gpt-5.6-sol"));
    }

    /// Every level must be reachable on real-shaped data. This is the property the
    /// max-scaled ramp did not have: against a 1.8B peak, a 50M day and a 500M day both
    /// landed in the top bucket and the calendar drew one flat colour.
    #[test]
    fn quartiles_populate_all_four_levels_on_a_skewed_year() {
        let sizes = [1u64, 5, 20, 40, 60, 90, 200, 1_000_000_000];
        let messages: Vec<_> = sizes
            .iter()
            .enumerate()
            .map(|(index, tokens)| {
                on(
                    date!(2026 - 08 - 01).replace_day(index as u8 + 1).unwrap(),
                    "m",
                    *tokens,
                )
            })
            .collect();
        let stats = summarise(&messages, &Prices::bundled());
        let cuts = stats.thresholds();

        let levels: Vec<usize> = sizes.iter().map(|t| Stats::level(&cuts, *t)).collect();
        for level in 1..=4 {
            assert!(
                levels.contains(&level),
                "level {level} unreachable: {levels:?}"
            );
        }
        assert_eq!(Stats::level(&cuts, 0), 0, "an empty day is never shaded");
    }

    /// The histograms have to account for every token, or a reader comparing them against
    /// the total finds a gap and cannot tell which figure to trust.
    #[test]
    fn the_histograms_account_for_every_token() {
        let messages = vec![
            on(date!(2026 - 08 - 02), "m", 10),
            on(date!(2026 - 08 - 03), "m", 25),
            on(date!(2026 - 08 - 03), "m", 5),
        ];
        let stats = summarise(&messages, &Prices::bundled());

        assert_eq!(stats.by_hour.iter().sum::<u64>(), 40);
        assert_eq!(stats.by_weekday.iter().sum::<u64>(), 40);
        // The fixture is fixed at noon UTC, and the 2nd of August 2026 is a Sunday.
        assert_eq!(stats.by_hour[12], 40);
        assert_eq!(stats.by_weekday[0], 10, "Sunday");
        assert_eq!(stats.by_weekday[1], 30, "Monday");
    }

    /// A year with one active day still has to bucket it somewhere.
    #[test]
    fn a_single_active_day_lands_in_a_level() {
        let stats = summarise(&[on(date!(2026 - 08 - 01), "m", 10)], &Prices::bundled());
        assert_eq!(Stats::level(&stats.thresholds(), 10), 4);
        assert_eq!(Stats::level(&[0; 3], 0), 0);
    }

    /// The heat map still reports the busiest day as a figure in its own right.
    #[test]
    fn the_busiest_day_is_the_maximum_not_an_average() {
        let messages = vec![
            on(date!(2026 - 08 - 01), "m", 1),
            on(date!(2026 - 08 - 02), "m", 1_000_000),
        ];
        assert_eq!(
            summarise(&messages, &Prices::bundled()).busiest(),
            1_000_000
        );
    }

    #[test]
    fn sessions_are_counted_once_however_many_messages_they_hold() {
        let mut messages = vec![on(date!(2026 - 08 - 01), "m", 1); 5];
        messages.push(on(date!(2026 - 08 - 02), "m", 1));
        let stats = summarise(&messages, &Prices::bundled());
        assert_eq!(stats.sessions, 2, "one id per day in the fixture");
    }

    /// A day of entirely unpriced models reports no cost, not a zero — the same
    /// distinction the tables make everywhere else.
    #[test]
    fn an_unpriced_day_has_no_cost_rather_than_a_zero_one() {
        let stats = summarise(
            &[on(date!(2026 - 08 - 01), "unlisted-model-1", 10)],
            &Prices::bundled(),
        );
        assert_eq!(stats.days[0].cost, None);
        assert_eq!(stats.days[0].tokens, 10);
    }
}
