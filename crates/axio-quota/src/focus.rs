//! The provider whose usable headline is closest to exhaustion owns the tray icon.

use crate::model::{ProviderId, UsageSnapshot};

/// Highest usable headline utilization wins. Equal utilization follows ProviderId's
/// stable declaration order, independent of the map or slice's iteration order.
/// Providers without a usable rate window cannot supply an honest percentage.
pub fn tray_focus(
    snapshots: &[(ProviderId, UsageSnapshot)],
) -> Option<&(ProviderId, UsageSnapshot)> {
    snapshots
        .iter()
        .filter_map(|entry| {
            entry
                .1
                .headline()
                .map(|window| (entry, window.used_percent))
        })
        .max_by(|(a, usage_a), (b, usage_b)| {
            usage_a
                .partial_cmp(usage_b)
                .expect("headline utilization is finite")
                .then_with(|| b.0.cmp(&a.0))
        })
        .map(|(entry, _)| entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RateWindow;

    fn snapshot(id: ProviderId, windows: &[(&str, f64)]) -> (ProviderId, UsageSnapshot) {
        let mut snapshot = UsageSnapshot::new(id);
        snapshot.windows = windows
            .iter()
            .map(|(label, used)| RateWindow::new(*label, *used))
            .collect();
        (id, snapshot)
    }

    #[test]
    fn the_provider_in_trouble_gets_the_icon() {
        let snapshots = vec![
            snapshot(ProviderId::Codex, &[("Weekly", 12.0)]),
            snapshot(ProviderId::Claude, &[("5h", 4.0), ("Weekly", 96.0)]),
        ];
        assert_eq!(tray_focus(&snapshots).unwrap().0, ProviderId::Claude);
    }

    #[test]
    fn ties_resolve_deterministically() {
        let snapshots = vec![
            snapshot(ProviderId::Claude, &[("5h", 0.0)]),
            snapshot(ProviderId::Codex, &[("Weekly", 0.0)]),
        ];
        assert_eq!(tray_focus(&snapshots).unwrap().0, ProviderId::Codex);
        let reversed: Vec<_> = snapshots.into_iter().rev().collect();
        assert_eq!(tray_focus(&reversed).unwrap().0, ProviderId::Codex);
    }

    #[test]
    fn a_provider_with_no_windows_never_wins() {
        let snapshots = vec![
            snapshot(ProviderId::Openrouter, &[]),
            snapshot(ProviderId::Codex, &[("Weekly", 3.0)]),
        ];
        assert_eq!(tray_focus(&snapshots).unwrap().0, ProviderId::Codex);
    }

    #[test]
    fn empty_or_unusable_snapshots_have_no_focus() {
        assert!(tray_focus(&[]).is_none());
        let mut invalid = snapshot(ProviderId::Claude, &[("Weekly", 0.0)]);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 101.0] {
            invalid.1.windows[0].used_percent = value;
            assert!(tray_focus(&[invalid.clone(), snapshot(ProviderId::Codex, &[])]).is_none());
        }
    }

    #[test]
    fn malformed_windows_do_not_hide_a_valid_headline() {
        let mut mixed = snapshot(ProviderId::Claude, &[("Weekly", 92.0), ("Invalid", 0.0)]);
        mixed.1.windows[1].used_percent = f64::NAN;
        let snapshots = [snapshot(ProviderId::Codex, &[("Weekly", 50.0)]), mixed];
        let focused = tray_focus(&snapshots).unwrap();
        assert_eq!(focused.0, ProviderId::Claude);
        assert_eq!(focused.1.headline().unwrap().used_percent, 92.0);
    }

    #[test]
    fn normalized_bounds_and_signed_zero_follow_the_same_policy() {
        let snapshots = [
            snapshot(ProviderId::Claude, &[("Weekly", 150.0)]),
            snapshot(ProviderId::Codex, &[("Weekly", 100.0)]),
        ];
        assert_eq!(tray_focus(&snapshots).unwrap().0, ProviderId::Codex);
        let zeros = [
            snapshot(ProviderId::Claude, &[("Weekly", 0.0)]),
            snapshot(ProviderId::Codex, &[("Weekly", -0.0)]),
        ];
        assert_eq!(tray_focus(&zeros).unwrap().0, ProviderId::Codex);
    }
}
