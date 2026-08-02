// The full window: four views over the same data.

import { useEffect, useState } from "react";
import {
  api,
  onUpdated,
  type CostSource,
  type Overview,
  type ProviderSetting,
} from "./api";
import { ProviderList } from "./Providers";
import { History } from "./History";

type Tab = "providers" | "history" | "cost" | "settings";

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
      <div className="tabs">
        {(["providers", "history", "cost", "settings"] as Tab[]).map((name) => (
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
        {tab === "settings" && <Settings onStatus={setStatus} />}
      </div>

      <div className="status">{status}</div>
    </div>
  );
}

function Cost() {
  const [sources, setSources] = useState<CostSource[]>([]);
  useEffect(() => {
    api.costSources().then(setSources);
  }, []);

  return (
    <>
      <h3>Not built yet</h3>
      <p className="muted">
        Quota endpoints report percentages, not money. Cost has to be computed
        from the session transcripts each CLI writes locally — token counts per
        model, priced. Showing $0.00 until then would be indistinguishable from a
        quiet week.
      </p>
      <p className="muted">Where the data would come from:</p>
      {sources.map((source) => (
        <div className="source" key={source.name}>
          <strong style={{ minWidth: "4rem" }}>{source.name}</strong>
          <span className={source.exists ? "found" : "muted"}>
            {source.exists ? "found" : "not present"}
          </span>
          <span className="muted">{source.path}</span>
        </div>
      ))}
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
