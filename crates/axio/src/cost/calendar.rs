//! `axio cost --calendar` — the year as a grid.
//!
//! The table answers *what did I spend*. This answers *when do I actually work*, which is
//! a shape rather than a number and so has to be drawn. Seven rows of weekdays, one column
//! per week, one cell per day, running back a year from the current week.
//!
//! Colour is decided by the caller from whether the sink is a terminal, the same rule the
//! rest of the CLI follows: `axio cost --calendar > out.txt` writes zero escape bytes, and
//! the calendar degrades to five distinguishable ASCII glyphs rather than to nothing.

use std::collections::BTreeMap;

use axio_cost::ScanReport;
use axio_cost::pricing::Prices;
use axio_cost::stats::{Stats, summarise};
use axio_cost::totals::render;
use time::{Date, Duration, OffsetDateTime, Weekday};

const WEEKS: i64 = 53;

/// A violet ramp in the 256-colour cube, plus a near-black for a day with nothing.
///
/// Violet because the terminal's own palette already spends red, yellow and green on
/// severity elsewhere in this CLI, and a busy day is not a warning. One hue at four steps
/// keeps the reading unambiguous: darker is less, and nothing else is being encoded.
const RAMP: [u8; 5] = [236, 53, 91, 128, 141];

/// The fallback when there is no colour, in increasing weight.
///
/// Five glyphs that stay distinguishable in a monospace font without colour — which is
/// what a redirected calendar is, and it should still be readable.
const GLYPHS: [char; 5] = ['·', '░', '▒', '▓', '█'];

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub(crate) fn calendar_command(report: &ScanReport, prices: &Prices, colour: bool) -> u8 {
    let stats = summarise(report.messages(), prices);
    if stats.days.is_empty() {
        println!("no sessions found on this machine");
        return 0;
    }

    let by_date: BTreeMap<Date, u64> = stats
        .days
        .iter()
        .map(|day| (day.date, day.tokens))
        .collect();
    // Levels come from the quartiles of the active days rather than from a ramp scaled
    // against the busiest one — see `Stats::thresholds` for what that ramp did to a year
    // whose peak is thirty times its median.
    let cuts = stats.thresholds();

    // The grid ends on the current week so that "now" is always the right-hand column.
    // Anchoring on the newest *data* instead would make a quiet fortnight look like the
    // calendar had stopped rather than like a quiet fortnight.
    let today = OffsetDateTime::now_utc().date();
    let last_sunday = today - Duration::days(days_since_sunday(today));
    let start = last_sunday - Duration::weeks(WEEKS - 1);

    println!("    {}", month_row(start));
    for weekday in 0..7 {
        let label = match weekday {
            1 => "Mon",
            3 => "Wed",
            5 => "Fri",
            // Seven labels on seven rows is noise; three is enough to orient by.
            _ => "   ",
        };
        let mut row = String::with_capacity(WEEKS as usize * 8);
        for week in 0..WEEKS {
            let date = start + Duration::weeks(week) + Duration::days(weekday);
            // Days past today are not "zero activity", they have not happened. They are
            // left blank so the last column does not read as a slump.
            if date > today {
                row.push(' ');
                continue;
            }
            let tokens = by_date.get(&date).copied().unwrap_or(0);
            row.push_str(&cell(Stats::level(&cuts, tokens), colour));
        }
        println!("{label} {row}");
    }

    let key: String = (0..5).map(|l| cell(l, colour)).collect();
    println!("    less {key} more");
    println!();
    summary(&stats);
    0
}

fn cell(level: usize, colour: bool) -> String {
    if colour {
        format!("\x1b[38;5;{}m█\x1b[0m", RAMP[level])
    } else {
        GLYPHS[level].to_string()
    }
}

/// The header, with each month's name over the first week that belongs to it.
///
/// Written into a fixed-width buffer rather than joined, because a label is three columns
/// wide and a week is one: the names have to overlap into their neighbours' space, and
/// only the ones with room to land get written at all.
fn month_row(start: Date) -> String {
    let mut row = vec![b' '; WEEKS as usize + 3];
    let mut previous = None;
    for week in 0..WEEKS {
        let date = start + Duration::weeks(week);
        if previous == Some(date.month()) {
            continue;
        }
        previous = Some(date.month());
        let name = MONTHS[u8::from(date.month()) as usize - 1].as_bytes();
        // Only where the previous label has cleared the space — two names three columns
        // wide cannot both start within three columns of each other.
        let at = week as usize;
        if row[at..at + 3].iter().all(|byte| *byte == b' ') {
            row[at..at + 3].copy_from_slice(name);
        }
    }
    String::from_utf8(row).unwrap_or_default()
}

fn summary(stats: &Stats) {
    println!("{:<16} {}", "total", render(&stats.totals.cost()));
    println!("{:<16} {}", "tokens", stats.totals.tokens.total());
    println!("{:<16} {}", "sessions", stats.sessions);
    println!("{:<16} {}", "active days", stats.active_days);
    println!(
        "{:<16} {} day{}",
        "current streak",
        stats.current_streak,
        plural(stats.current_streak)
    );
    println!(
        "{:<16} {} day{}",
        "longest streak",
        stats.longest_streak,
        plural(stats.longest_streak)
    );
    if let Some(model) = &stats.top_model {
        println!("{:<16} {model}", "most used");
    }
    if let Some(busiest) = busiest_day(stats) {
        println!("{:<16} {busiest}", "busiest day");
    }
}

fn busiest_day(stats: &Stats) -> Option<String> {
    let day = stats.days.iter().max_by_key(|day| day.tokens)?;
    Some(format!("{} ({} tokens)", day.date, day.tokens))
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// How far back the Sunday is. `time`'s week numbering starts on Monday, and the grid is
/// drawn Sunday-first, so this cannot be `weekday as i64`.
fn days_since_sunday(date: Date) -> i64 {
    match date.weekday() {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    /// Every level the bucketing can return must have a cell to draw, or the calendar
    /// panics on the year that finally reaches the top bucket.
    #[test]
    fn every_level_has_a_cell() {
        for level in 0..RAMP.len() {
            assert!(!cell(level, true).is_empty());
            assert!(!cell(level, false).is_empty());
        }
        assert_eq!(RAMP.len(), GLYPHS.len());
    }

    #[test]
    fn sunday_is_the_first_row() {
        assert_eq!(days_since_sunday(date!(2026 - 08 - 02)), 0, "a Sunday");
        assert_eq!(
            days_since_sunday(date!(2026 - 08 - 03)),
            1,
            "the Monday after"
        );
        assert_eq!(
            days_since_sunday(date!(2026 - 08 - 01)),
            6,
            "the Saturday before"
        );
    }

    /// Every month in the window should be named, and no name may overwrite another.
    #[test]
    fn the_month_row_names_months_without_colliding() {
        let row = month_row(date!(2025 - 08 - 03));
        assert!(row.contains("Aug"));
        assert!(row.contains("Jan"));
        assert!(row.contains("Jul"));
        assert_eq!(row.len(), WEEKS as usize + 3);
    }

    /// A redirected calendar has to stay readable, so the plain form must not be blank.
    #[test]
    fn without_colour_the_cells_are_glyphs_and_carry_no_escapes() {
        for l in 0..5 {
            let plain = cell(l, false);
            assert!(!plain.contains('\x1b'));
            assert_ne!(plain, " ");
        }
        assert!(cell(4, true).contains('\x1b'));
    }
}
