//! The columns that are computed rather than counted.
//!
//! Split out of `cost` when it outgrew the file-length gate. These four share a property
//! worth keeping together: each is a ratio, and each had to decide what to do when its
//! denominator is missing or means different things to different vendors.

use axio_cost::totals::Totals;

/// Cache reads as a share of the row's tokens.
///
/// A share and not the multiple of fresh input that this first reported. The multiple is
/// arithmetically fine and practically useless, because the vendors do not mean the same
/// thing by "input": OpenAI counts the whole prompt, Anthropic counts only the part that
/// missed cache. Measured live, that put gpt-5.6-sol at 31x and claude-opus-5 at 121,077x
/// — two numbers in one column that cannot be compared to each other.
///
/// A share has the same meaning whoever reported it, and answers the question the column
/// exists for: how much of this volume was billed at the cheap rate.
pub(super) fn cache_ratio(totals: &Totals) -> String {
    let total = totals.tokens.total();
    if total == 0 {
        return "-".to_string();
    }
    format!("{:.0}%", totals.tokens.cache_read as f64 / total as f64 * 100.0)
}

/// Dollars per million tokens, blended across every rate the row's models charge.
///
/// Divided by the tokens that were priced rather than by all of them: dividing a partial
/// cost by a whole volume understates the rate by exactly the share that had no price.
pub(super) fn per_million(totals: &Totals) -> String {
    let priced = totals.tokens.total().saturating_sub(totals.unpriced_tokens);
    match (spend_opt(totals), priced) {
        (Some(dollars), priced) if priced > 0 => {
            format!("{:.2}", dollars / (priced as f64 / 1_000_000.0))
        }
        _ => "-".to_string(),
    }
}

pub(super) fn share(totals: &Totals, grand: f64) -> String {
    match spend_opt(totals) {
        Some(dollars) if grand > 0.0 => format!("{:.1}%", dollars / grand * 100.0),
        _ => "-".to_string(),
    }
}

fn spend_opt(totals: &Totals) -> Option<f64> {
    totals.cost().partial().map(|(dollars, _)| dollars)
}
