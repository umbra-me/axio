// The panel that opens against the tray icon.
//
// This is the faithful port of the popover the macOS original shows. It is deliberately
// not a menu: bars and countdowns are the point, and a native menu can only hold strings.

import { useEffect, useState } from "react";
import { api, onUpdated, type Overview } from "./api";
import { ProviderList } from "./Providers";

export function Flyout() {
  const [data, setData] = useState<Overview>({
    providers: [],
    refreshing: true,
  });

  useEffect(() => {
    const load = () => api.overview().then(setData);
    load();
    const unlisten = onUpdated(load);
    // Countdowns are rendered from a timestamp, so without a tick "resets in 2h" would
    // stay "2h" until the next probe five minutes later.
    const timer = setInterval(load, 30_000);
    return () => {
      unlisten.then((off) => off());
      clearInterval(timer);
    };
  }, []);

  return (
    <div className="flyout">
      <div className="flyout-head">
        <span className="brand">axio quota</span>
        <div className="spacer" />
        {data.refreshing && <span className="muted">refreshing…</span>}
      </div>

      <div className="flyout-body">
        <ProviderList providers={data.providers} refreshing={data.refreshing} />
      </div>

      <div className="flyout-foot">
        <button onClick={() => api.refresh()} disabled={data.refreshing}>
          Refresh
        </button>
        <button onClick={() => api.openMainWindow()}>Open</button>
        <div className="spacer" />
        <button onClick={() => api.quit()}>Quit</button>
      </div>
    </div>
  );
}
