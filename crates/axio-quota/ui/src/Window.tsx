// The full window: four views over the same data.

import { useEffect, useState } from "react";
import {
  api,
  onCostUpdated,
  onUpdated,
  type CostGroup,
  type CostReport,
  type CostRow,
  type Overview,
  type ProviderSetting,
  type Storage,
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

  // The scan runs on a worker and publishes twice — the saved scan, then the live one.
  // Without this the view would show last session's figures until something else made it
  // reload, which is exactly the "it isn't saving properly" it was meant to fix.
  useEffect(() => {
    const unlisten = onCostUpdated(() => load(group));
    return () => {
      unlisten.then((off) => off());
    };
  }, [group]);

  // Returns as soon as the worker starts. The result arrives via `cost://updated`, so
  // there is nothing to await here beyond the request itself.
  const rescan = () => {
    setBusy(true);
    api.refreshCost().finally(() => setBusy(false));
  };

  // Only before anything at all has been read. Once a saved scan is published this view
  // shows real figures and says a fresher scan is running, rather than hiding both.
  if (!report || report.loading) {
    return (
      <>
        <h3>Reading session transcripts…</h3>
        <p className="muted">
          First run walks every agent's logs on this machine. The result is saved, so
          later launches show it straight away.
        </p>
      </>
    );
  }

  const groups: CostGroup[] = [
    "model",
    "provider",
    "harness",
    "hour",
    "day",
    "week",
    "month",
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
        <button className="tab" onClick={rescan} disabled={busy || report.scanning}>
          {busy || report.scanning ? "scanning…" : "rescan"}
        </button>
      </div>

      <table className="cost">
        <thead>
          <tr>
            <th>{group}</th>
            <th className="num">messages</th>
            <th className="num">tokens</th>
            <th className="num" title="Cache reads as a share of this row's tokens. A share rather than a multiple of fresh input, because the vendors disagree about what input means.">
              cached
            </th>
            <th className="num" title="Blended dollars per million priced tokens — what this row actually cost per unit, across every rate its models charge.">
              $/1M
            </th>
            <th className="num">share</th>
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
              <td className="num muted">{percent(row.cacheRatio)}</td>
              <td className="num muted">
                {row.perMillion !== null ? `$${row.perMillion.toFixed(2)}` : "—"}
              </td>
              <td className="num muted">{percent(row.share)}</td>
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
            <td className="num muted">{percent(report.total.cacheRatio)}</td>
            <td className="num muted">
              {report.total.perMillion !== null
                ? `$${report.total.perMillion.toFixed(2)}`
                : "—"}
            </td>
            <td className="num muted">100%</td>
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

function percent(value: number | null): string {
  if (value === null) return "—";
  // Whole percent: these are proportions read at a glance beside a column of money, and a
  // decimal place here competes with the figure that matters.
  return `${Math.round(value * 100)}%`;
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
  const [storage, setStorage] = useState<Storage | null>(null);
  const [agents, setAgents] = useState<CostReport["agents"]>([]);

  useEffect(() => {
    api.settings().then(setSettings);
    const load = () => {
      api.storage().then(setStorage);
      api.costReport("harness").then((report) => setAgents(report.agents));
    };
    load();
    const unlisten = onCostUpdated(load);
    return () => {
      unlisten.then((off) => off());
    };
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
            <div className="fields">
              <input
                type="password"
                placeholder="API key"
                value={entry.apiKey ?? ""}
                onChange={(event) =>
                  update(entry.id, { apiKey: event.target.value })
                }
              />
              {entry.takesWorkspaceId && (
                <input
                  type="text"
                  placeholder={entry.workspaceHint}
                  value={entry.workspaceId ?? ""}
                  onChange={(event) =>
                    update(entry.id, { workspaceId: event.target.value })
                  }
                />
              )}
              <div className="hint">{entry.hint}</div>
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

      <h4 className="section">Agents read for cost</h4>
      <p className="note">
        Found by walking each agent's own log directory. Nothing is configured here —
        an agent appears the moment it writes its first session.
      </p>
      <div className="agents">
        {agents.map((agent) => (
          <span
            key={agent.name}
            className={`agent ${agent.present ? "found" : ""}`}
            title={
              agent.present
                ? `${agent.files.toLocaleString()} files, ${agent.messages.toLocaleString()} messages`
                : "Not installed on this machine"
            }
          >
            {agent.name}
            {agent.present && (
              <b>{agent.messages > 0 ? agent.messages.toLocaleString() : "0"}</b>
            )}
          </span>
        ))}
      </div>

      <h4 className="section">Files</h4>
      {storage && (
        <dl className="paths">
          <div>
            <dt>Settings</dt>
            <dd title={storage.configPath}>{storage.configPath}</dd>
          </div>
          <div>
            <dt>Quota history</dt>
            <dd title={storage.historyPath}>{storage.historyPath}</dd>
          </div>
          <div>
            <dt>Saved scan</dt>
            <dd title={storage.scan.path}>
              {storage.scan.bytes > 0
                ? `${storage.scan.path} — ${megabytes(storage.scan.bytes)}`
                : "not saved yet"}
            </dd>
          </div>
        </dl>
      )}
      <p className="note">
        The saved scan is machine-local and rebuildable: deleting it costs one rescan,
        not any history.
      </p>
      <div style={{ display: "flex", gap: "0.4rem" }}>
        <button
          disabled={storage?.scan.scanning}
          onClick={async () => {
            await api.refreshCost();
            onStatus("Rescanning transcripts…");
          }}
        >
          {storage?.scan.scanning ? "Scanning…" : "Rescan now"}
        </button>
      </div>
    </>
  );
}

function megabytes(bytes: number): string {
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
