// The full window: four views over the same data.

import { useEffect, useState } from "react";
import {
  api,
  onUpdated,
  type CostGroup,
  type CostReport,
  type CostRow,
  type Overview,
  type ProviderSetting,
} from "./api";
import { ProviderList } from "./Providers";
import { History } from "./History";
import { Stats } from "./Stats";

type Tab = "providers" | "history" | "cost" | "stats" | "settings";

export function Window() {
  const [tab, setTab] = useState<Tab>("providers");
  const [data, setData] = useState<Overview>({
    providers: [],
    refreshing: true,
  });
  const [status, setStatus] = useState("");

  useEffect(() => {
    const load = () => api.overview().then(setData);
    load();
    const unlisten = onUpdated(load);
    const timer = setInterval(load, 30_000);
    return () => {
      unlisten.then((off) => off());
      clearInterval(timer);
    };
  }, []);

  return (
    <div className="app">
      <TitleBar />
      <div className="tabs">
        {(["providers", "history", "cost", "stats", "settings"] as Tab[]).map((name) => (
          <button
            key={name}
            className="tab"
            aria-selected={tab === name}
            onClick={() => setTab(name)}
          >
            {name[0].toUpperCase() + name.slice(1)}
          </button>
        ))}
        <div className="spacer" />
        <button onClick={() => api.refresh()} disabled={data.refreshing}>
          {data.refreshing ? "Refreshing…" : "Refresh"}
        </button>
      </div>

      <div className="body">
        {tab === "providers" && (
          <ProviderList
            providers={data.providers}
            refreshing={data.refreshing}
          />
        )}
        {tab === "history" && <History />}
        {tab === "cost" && <Cost />}
        {tab === "stats" && <Stats />}
        {tab === "settings" && <Settings onStatus={setStatus} />}
      </div>

      <div className="status">{status}</div>
    </div>
  );
}

/// The window's own titlebar, because the OS one was removed.
///
/// `data-tauri-drag-region` is what lets a frameless window be moved, and it has to sit on
/// the inert parts only — a button inside the region drags instead of pressing, which is
/// the single failure mode of every custom titlebar.
function TitleBar() {
  return (
    <div className="titlebar" data-tauri-drag-region>
      <span className="brand" data-tauri-drag-region>
        <i />
        <b>axio</b> quota
      </span>
      <div className="spacer" data-tauri-drag-region />
      <div className="win-controls">
        <button onClick={() => api.minimizeWindow()} aria-label="Minimise" title="Minimise">
          <svg viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1" y1="5" x2="9" y2="5" />
          </svg>
        </button>
        {/* Close hides the window. The tray icon is the app; quitting is a menu item. */}
        <button
          className="close"
          onClick={() => api.closeWindow()}
          aria-label="Close"
          title="Close to tray"
        >
          <svg viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" />
            <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" />
          </svg>
        </button>
      </div>
    </div>
  );
}

function Cost() {
  const [group, setGroup] = useState<CostGroup>("model");
  const [report, setReport] = useState<CostReport | null>(null);
  const [busy, setBusy] = useState(false);

  const load = (next: CostGroup) => {
    setBusy(true);
    api
      .costReport(next)
      .then(setReport)
      .finally(() => setBusy(false));
  };

  useEffect(() => {
    load(group);
    // Grouping is regrouping a cached scan, not rescanning, so this is cheap.
  }, [group]);

  const rescan = () => {
    setBusy(true);
    api
      .refreshCost()
      .then(() => api.costReport(group))
      .then(setReport)
      .finally(() => setBusy(false));
  };

  // The first scan reads every transcript on disk and takes tens of seconds. Saying so is
  // the whole difference between "working" and "broken" from the other side of a window.
  if (!report || report.loading) {
    return (
      <>
        <h3>Reading session transcripts…</h3>
        <p className="muted">
          First run walks every agent's logs on this machine. Later views regroup a cached
          scan and are instant.
        </p>
      </>
    );
  }

  const groups: CostGroup[] = [
    "model",
    "provider",
    "harness",
    "day",
    "workspace",
    "session",
  ];

  return (
    <>
      <div className="tabs" role="tablist" aria-label="Group cost by">
        {groups.map((name) => (
          <button
            key={name}
            className="tab"
            role="tab"
            aria-selected={group === name}
            onClick={() => setGroup(name)}
          >
            {name}
          </button>
        ))}
        <button className="tab" onClick={rescan} disabled={busy}>
          {busy ? "scanning…" : "rescan"}
        </button>
      </div>

      <table className="cost">
        <thead>
          <tr>
            <th>{group}</th>
            <th className="num">messages</th>
            <th className="num">tokens</th>
            <th className="num">cost</th>
          </tr>
        </thead>
        <tbody>
          {report.rows.map((row) => (
            <tr key={row.key}>
              <td className="key" title={row.key}>
                {row.key}
              </td>
              <td className="num">{row.messages.toLocaleString()}</td>
              <td className="num">{row.tokens.toLocaleString()}</td>
              <td className="num">
                <Money row={row} />
              </td>
            </tr>
          ))}
        </tbody>
        <tfoot>
          <tr>
            <td>total</td>
            <td className="num">{report.total.messages.toLocaleString()}</td>
            <td className="num">{report.total.tokens.toLocaleString()}</td>
            <td className="num">
              <Money row={report.total} />
            </td>
          </tr>
        </tfoot>
      </table>

      {report.unpricedModels.length > 0 && (
        <p className="muted">
          No published rate for {report.unpricedModels.join(", ")}. Their tokens are
          counted and their cost is left out rather than guessed.
        </p>
      )}

      <p className="muted">
        {report.agents.filter((a) => a.present).length} of {report.agents.length} agents
        found on this machine.
      </p>
    </>
  );
}

/// A figure, never claiming more than it knows.
///
/// `costUsd` is null when too little of the row could be priced for a number to mean
/// anything; a partly-priced row shows its coverage beside the figure. Both cases exist
/// because a total assembled from partly-priced data is not a cost — see the Rust side.
function Money({ row }: { row: CostRow }) {
  if (row.costUsd === null) {
    return <span className="muted">unpriced</span>;
  }
  const dollars = `$${row.costUsd.toFixed(2)}`;
  if (row.coverage >= 0.999) {
    return <>{dollars}</>;
  }
  // Floored, so 99.96% never reads as a confident 100%.
  const percent = (Math.floor(row.coverage * 1000) / 10).toFixed(1);
  return (
    <>
      {dollars} <span className="muted">({percent}% priced)</span>
    </>
  );
}

function Settings({ onStatus }: { onStatus: (text: string) => void }) {
  const [settings, setSettings] = useState<ProviderSetting[]>([]);
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    api.settings().then(setSettings);
  }, []);

  const update = (id: string, patch: Partial<ProviderSetting>) => {
    setSettings((current) =>
      current.map((entry) =>
        entry.id === id ? { ...entry, ...patch } : entry,
      ),
    );
    setDirty(true);
  };

  return (
    <>
      {settings.map((entry) => (
        <div className="setting" key={entry.id}>
          <div className="setting-row">
            <input
              type="checkbox"
              checked={entry.enabled}
              onChange={(event) =>
                update(entry.id, { enabled: event.target.checked })
              }
            />
            <strong>{entry.name}</strong>
          </div>
          {entry.takesApiKey ? (
            <div className="setting-row" style={{ paddingLeft: "1.5rem" }}>
              <input
                type="password"
                placeholder={entry.hint}
                value={entry.apiKey ?? ""}
                onChange={(event) =>
                  update(entry.id, { apiKey: event.target.value })
                }
              />
            </div>
          ) : (
            <div className="hint">{entry.hint}</div>
          )}
        </div>
      ))}

      <div style={{ display: "flex", gap: "0.4rem", marginTop: "0.8rem" }}>
        <button
          disabled={!dirty}
          onClick={async () => {
            const path = await api.saveSettings(settings);
            setDirty(false);
            onStatus(`Saved to ${path}`);
          }}
        >
          Save
        </button>
        <button
          disabled={!dirty}
          onClick={async () => {
            setSettings(await api.settings());
            setDirty(false);
            onStatus("Reverted to the file on disk");
          }}
        >
          Discard
        </button>
        {dirty && <span className="muted">Unsaved changes</span>}
      </div>
    </>
  );
}
