// Provider rows, shared by the window and the flyout.
//
// One component for both surfaces on purpose: the flyout is not a summary, it is the same
// information in a narrower column. Two components would drift.

import type { ProviderView, RateWindow } from "./api";
import { resetText, severity } from "./api";

export function Meter({ window }: { window: RateWindow }) {
  const used = Math.min(100, Math.max(0, window.used_percent));
  return (
    <div className="window">
      <div className="window-label" title={window.label}>
        {window.label}
      </div>
      <div className="window-reset">
        {Math.round(used)}%{" "}
        <span className="muted">{resetText(window.resets_at)}</span>
      </div>
      <div className="meter">
        <div
          className={`meter-fill ${severity(used)}`}
          style={{ width: `${used}%` }}
        />
      </div>
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
