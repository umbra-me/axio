//! When to probe again.
//!
//! A fixed interval is wrong at both ends and the old five-minute constant said so in its
//! own doc comment: far more often than a weekly window needs, far too slow to watch a
//! session window drain under heavy use. The cost of being wrong is asymmetric, which is
//! what makes a schedule worth computing rather than choosing. Probing too often wastes a
//! request against a rate limit that exists to stop exactly that. Probing too rarely means
//! a window empties, resets, and refills without the tray ever showing it — the one job.
//!
//! So the interval is derived from the two things that decide how fast the answer can
//! change: how close a window is to its reset, and how close it is to full.

use std::time::Duration;

use crate::Results;

/// The floor. Every vendor here rate-limits the usage endpoint itself, and a tray icon
/// that gets itself throttled has made its own reading unavailable.
const FASTEST: Duration = Duration::from_secs(60);

/// The ceiling, for an account with nothing happening. Long enough to be nearly free,
/// short enough that a window opened after lunch is not showing breakfast's numbers.
const SLOWEST: Duration = Duration::from_secs(30 * 60);

/// A window this close to resetting is worth watching: the number is about to change
/// completely, and that is the moment a countdown in the tray is most wrong.
const NEAR_RESET: i64 = 10 * 60;

/// Above this, the remaining headroom is small enough that the gap between readings
/// matters — the difference between 88% and 96% is the difference between starting a long
/// task and not.
const BUSY_PERCENT: f64 = 75.0;

/// How the app decides its own refresh cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// Derive the interval from what the last probe returned.
    Adaptive,
    /// A fixed interval, in seconds, as the user chose it.
    Every(u64),
    /// Never on a timer; the Refresh button only.
    Manual,
}

impl Cadence {
    /// Parse the stored setting. Anything unrecognised is adaptive, which is the default
    /// and the only value that is right for someone who never opened Settings.
    pub fn parse(raw: Option<&str>) -> Cadence {
        match raw.map(str::trim) {
            Some("manual") => Cadence::Manual,
            Some(other) => match other.strip_suffix('s').unwrap_or(other).parse::<u64>() {
                Ok(seconds) if seconds > 0 => Cadence::Every(seconds.max(FASTEST.as_secs())),
                _ => Cadence::Adaptive,
            },
            None => Cadence::Adaptive,
        }
    }

    pub fn as_str(self) -> String {
        match self {
            Cadence::Adaptive => "adaptive".to_string(),
            Cadence::Manual => "manual".to_string(),
            Cadence::Every(seconds) => seconds.to_string(),
        }
    }
}

/// How long to wait before probing again, given what the last probe found.
///
/// `now_unix` is passed rather than read so the policy is testable: a schedule that can
/// only be checked by waiting is a schedule nobody checks.
pub fn interval(cadence: Cadence, results: &Results, now_unix: i64) -> Option<Duration> {
    match cadence {
        Cadence::Manual => None,
        Cadence::Every(seconds) => Some(Duration::from_secs(seconds)),
        Cadence::Adaptive => Some(adaptive(results, now_unix)),
    }
}

fn adaptive(results: &Results, now_unix: i64) -> Duration {
    let mut soonest_reset: Option<i64> = None;
    let mut busiest = 0.0f64;

    for (_, outcome) in results {
        // A provider that failed is not evidence of calm. It is left out of both figures
        // rather than counted as 0% used, which would slow the loop down exactly when
        // something has gone wrong and a retry is what is wanted.
        let Ok(snapshot) = outcome else {
            return FASTEST.max(Duration::from_secs(120));
        };
        for window in &snapshot.windows {
            busiest = busiest.max(window.used_percent);
            if let Some(at) = window.resets_at {
                let seconds = at.unix_timestamp() - now_unix;
                // A reset in the past is a stale snapshot, not an imminent event.
                if seconds > 0 {
                    soonest_reset = Some(soonest_reset.map_or(seconds, |best: i64| best.min(seconds)));
                }
            }
        }
    }

    if soonest_reset.is_some_and(|seconds| seconds <= NEAR_RESET) || busiest >= 90.0 {
        return FASTEST;
    }
    if busiest >= BUSY_PERCENT {
        return Duration::from_secs(2 * 60);
    }

    // Otherwise pace against the nearest reset: there is no point sampling a weekly window
    // every five minutes, and no harm in sampling a five-hour one every few. A twentieth of
    // the remaining time gives roughly twenty readings before the window turns over, which
    // is enough to draw the History chart's shape.
    match soonest_reset {
        Some(seconds) => Duration::from_secs((seconds as u64 / 20).max(FASTEST.as_secs()))
            .min(SLOWEST),
        // Nothing reported a reset at all, so there is nothing to pace against.
        None => Duration::from_secs(5 * 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProviderId, RateWindow, UsageSnapshot};
    use time::OffsetDateTime;

    const NOW: i64 = 1_785_734_400;

    fn results(windows: Vec<RateWindow>) -> Results {
        let mut snapshot = UsageSnapshot::new(ProviderId::Codex);
        snapshot.windows = windows;
        vec![(ProviderId::Codex, Ok(snapshot))]
    }

    fn window(used: f64, resets_in: Option<i64>) -> RateWindow {
        RateWindow::new("w", used).with_reset(
            resets_in.map(|seconds| {
                OffsetDateTime::from_unix_timestamp(NOW + seconds).expect("valid")
            }),
        )
    }

    #[test]
    fn a_reset_about_to_happen_pulls_the_interval_to_the_floor() {
        let soon = results(vec![window(5.0, Some(120))]);
        assert_eq!(interval(Cadence::Adaptive, &soon, NOW), Some(FASTEST));
    }

    #[test]
    fn a_nearly_full_window_pulls_the_interval_in() {
        let full = results(vec![window(95.0, Some(60 * 60 * 24))]);
        assert_eq!(interval(Cadence::Adaptive, &full, NOW), Some(FASTEST));
        let busy = results(vec![window(80.0, Some(60 * 60 * 24))]);
        assert_eq!(
            interval(Cadence::Adaptive, &busy, NOW),
            Some(Duration::from_secs(120))
        );
    }

    /// A quiet weekly window does not need twelve readings an hour.
    #[test]
    fn a_distant_reset_relaxes_the_interval_to_the_ceiling() {
        let quiet = results(vec![window(3.0, Some(60 * 60 * 24 * 7))]);
        assert_eq!(interval(Cadence::Adaptive, &quiet, NOW), Some(SLOWEST));
    }

    /// Twenty readings before a window turns over is enough to draw its shape.
    #[test]
    fn a_five_hour_window_is_sampled_about_twenty_times() {
        let session = results(vec![window(20.0, Some(60 * 60 * 5))]);
        assert_eq!(
            interval(Cadence::Adaptive, &session, NOW),
            Some(Duration::from_secs(900))
        );
    }

    /// A reset stamp in the past means the snapshot is stale, not that a reset is due.
    #[test]
    fn a_reset_already_past_is_ignored() {
        let stale = results(vec![window(10.0, Some(-3600))]);
        assert_eq!(
            interval(Cadence::Adaptive, &stale, NOW),
            Some(Duration::from_secs(300)),
            "falls back to the no-reset default rather than the floor"
        );
    }

    /// A failing provider should be retried soon, not treated as an idle one.
    #[test]
    fn an_error_shortens_the_interval_rather_than_lengthening_it() {
        let failed: Results = vec![(
            ProviderId::Codex,
            Err(crate::error::ProbeError::NotConfigured("no key".into())),
        )];
        assert_eq!(
            interval(Cadence::Adaptive, &failed, NOW),
            Some(Duration::from_secs(120))
        );
    }

    #[test]
    fn manual_never_schedules_and_a_fixed_choice_is_honoured() {
        assert_eq!(interval(Cadence::Manual, &results(vec![]), NOW), None);
        assert_eq!(
            interval(Cadence::Every(900), &results(vec![]), NOW),
            Some(Duration::from_secs(900))
        );
    }

    /// The floor applies to a chosen interval too: a five-second setting would get the
    /// user rate-limited by their own provider.
    #[test]
    fn a_setting_below_the_floor_is_raised_to_it() {
        assert_eq!(Cadence::parse(Some("5")), Cadence::Every(60));
        assert_eq!(Cadence::parse(Some("900")), Cadence::Every(900));
        assert_eq!(Cadence::parse(Some("300s")), Cadence::Every(300));
        assert_eq!(Cadence::parse(Some("manual")), Cadence::Manual);
        assert_eq!(Cadence::parse(Some("nonsense")), Cadence::Adaptive);
        assert_eq!(Cadence::parse(None), Cadence::Adaptive);
    }

    #[test]
    fn a_cadence_round_trips_through_its_stored_form() {
        for cadence in [Cadence::Adaptive, Cadence::Manual, Cadence::Every(600)] {
            assert_eq!(Cadence::parse(Some(&cadence.as_str())), cadence);
        }
    }
}
