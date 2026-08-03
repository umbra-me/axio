// Provider rows, shared by the window and the flyout.
//
// One component for both surfaces on purpose: the flyout is not a summary, it is the same
// information in a narrower column. Two components would drift.

import type { ProviderView, RateWindow } from "./api";
import { elapsedFraction, pace, resetText, severity } from "./api";

/// One quota window, drawn as a rail.
///
/// The fill is how much is gone. The tick is how far through the period we are. Reading
/// one against the other answers the question a bare percentage cannot: at this rate, does
/// the quota outlast the window? Both surfaces use this — the flyout is not a summary of
/// the window, it is the same instrument in a narrower column.
export function Meter({ window }: { window: RateWindow }) {
  const used = Math.min(100, Math.max(0, window.used_percent));
  const elapsed = elapsedFraction(window);
  const running = pace(window);

  return (
    <div className="window">
      <div className="window-label" title={window.label}>
        {window.label}
      </div>
      <div className="window-reset">
        {Math.round(used)}%{" "}
        <span className="muted">{resetText(window.resets_at)}</span>
      </div>
      <div
        className="rail"
        role="meter"
        aria-valuenow={Math.round(used)}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={`${window.label}: ${Math.round(used)}% used${running ? `, ${running.label}` : ""}`}
      >
        <div
          className={`rail-fill ${severity(used)}`}
          style={{ width: `${used}%` }}
        />
        {elapsed !== null && (
          <div
            className="rail-tick"
            style={{ left: `${elapsed * 100}%` }}
            title="where the clock is"
          />
        )}
      </div>
      {running && (
        <div className={`pace ${running.over ? "over" : ""}`}>
          {running.label}
        </div>
      )}
    </div>
  );
}

export function ProviderRows({ provider }: { provider: ProviderView }) {
  return (
    <div className="provider">
      <div className="provider-head">
        <span className="provider-name">{provider.name}</span>
        {provider.snapshot?.plan && (
          <span className="plan">{provider.snapshot.plan}</span>
        )}
      </div>

      {provider.error && <div className="error">{provider.error}</div>}

      {provider.snapshot?.windows.map((window) => (
        <Meter key={window.label} window={window} />
      ))}

      {provider.snapshot?.credits?.balance != null && (
        <div className="window">
          <div className="window-label">Credits</div>
          <div className="window-reset">
            {provider.snapshot.credits.balance.toFixed(2)}{" "}
            <span className="muted">remaining</span>
          </div>
        </div>
      )}
    </div>
  );
}

export function ProviderList({
  providers,
  refreshing,
}: {
  providers: ProviderView[];
  refreshing: boolean;
}) {
  if (providers.length === 0) {
    return (
      <div className="empty">
        {refreshing
          ? "Reading provider limits…"
          : "No provider reported. Check Settings, or run `axio quota --diagnose`."}
      </div>
    );
  }
  return (
    <>
      {providers.map((provider) => (
        <ProviderRows key={provider.id} provider={provider} />
      ))}
    </>
  );
}
