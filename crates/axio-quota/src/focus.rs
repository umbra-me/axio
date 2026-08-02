//! Which provider the single tray icon shows.
//!
//! macOS gives CodexBar a way out of this decision: it can put one status item per provider
//! in the menu bar. The Windows notification area can technically hold several icons, but
//! they collapse into the overflow chevron and users pin one. So with three providers
//! enabled, one number gets the pixel — and picking the wrong one makes the icon a liar.
//!
//! `UsageSnapshot::headline` already answers this *within* one provider (the window closest
//! to exhaustion). This is the cross-provider version, and it is a product decision rather
//! than a mechanical one — see the TODO below.

use crate::model::{ProviderId, UsageSnapshot};

/// The snapshot whose headline window the tray icon should render.
///
/// TODO: implement. Some approaches, none obviously correct:
///
/// - **Highest utilization wins.** Simple and honest about danger, but the icon jitters
///   between providers as numbers cross, and a provider you barely use can dominate it.
/// - **Pinned provider, others in the menu.** Stable and predictable, but silently hides
///   the provider that is actually about to run out.
/// - **Pinned by default, escalate on a threshold.** Shows your primary provider until
///   something crosses (say) 90%, then switches and stays until it recovers. Needs
///   hysteresis or it flaps at the boundary.
/// - **Soonest reset wins.** Optimizes for "can I start this task now" rather than for
///   "how much is left", which is arguably the real question.
///
/// Ties matter: two providers at exactly 0% on a fresh morning should not make the icon
/// depend on `HashMap` iteration order.
pub fn tray_focus(
    snapshots: &[(ProviderId, UsageSnapshot)],
) -> Option<&(ProviderId, UsageSnapshot)> {
    // Placeholder so the crate builds: first enabled provider, ignoring usage entirely.
    snapshots.first()
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
    #[ignore = "spec for tray_focus; un-ignore once it is implemented"]
    fn the_provider_in_trouble_gets_the_icon() {
        let snapshots = vec![
            snapshot(ProviderId::Codex, &[("Weekly", 12.0)]),
            snapshot(ProviderId::Claude, &[("5h", 4.0), ("Weekly", 96.0)]),
        ];
        assert_eq!(tray_focus(&snapshots).unwrap().0, ProviderId::Claude);
    }

    #[test]
    #[ignore = "spec for tray_focus; un-ignore once it is implemented"]
    fn ties_resolve_deterministically() {
        let snapshots = vec![
            snapshot(ProviderId::Claude, &[("5h", 0.0)]),
            snapshot(ProviderId::Codex, &[("Weekly", 0.0)]),
        ];
        let first = tray_focus(&snapshots).unwrap().0;
        let again = tray_focus(&snapshots).unwrap().0;
        assert_eq!(first, again);
    }

    #[test]
    #[ignore = "spec for tray_focus; un-ignore once it is implemented"]
    fn a_provider_with_no_windows_never_wins() {
        let snapshots = vec![
            snapshot(ProviderId::Openrouter, &[]),
            snapshot(ProviderId::Codex, &[("Weekly", 3.0)]),
        ];
        assert_eq!(tray_focus(&snapshots).unwrap().0, ProviderId::Codex);
    }
}
