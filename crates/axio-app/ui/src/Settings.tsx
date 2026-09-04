import { useEffect, useState } from "react";
import { api, describe, type AppSettings, type HostedView, type ProjectView, type SettingsView } from "./bridge";
import { chords, label, type Action } from "./shortcuts";
import { IconClose, IconRepo } from "./icons";

// The settings surface.
//
// A dialog with sections down its left, like every desktop application's, and
// nothing clever in it: the values are Rust's, this edits a copy and hands the
// whole thing back on Save. Appearance is previewed live while the dialog is
// open — `applyAppearance` is the same function the window calls on start — so
// what a change looks like is seen before it is kept; Cancel puts it back.

type Section = "appearance" | "terminal" | "agents" | "model" | "repositories" | "keyboard";

const SECTIONS: { id: Section; title: string }[] = [
  { id: "appearance", title: "Appearance" },
  { id: "terminal", title: "Terminal" },
  { id: "agents", title: "Agents" },
  { id: "model", title: "Model" },
  { id: "repositories", title: "Repositories" },
  { id: "keyboard", title: "Keyboard" },
];

/** What each chord does, in words the keyboard section can show. */
const ACTIONS: { action: Action; title: string }[] = [
  { action: "newSession", title: "New session" },
  { action: "newTerminal", title: "New terminal for the first agent" },
  { action: "addRepository", title: "Add a repository" },
  { action: "palette", title: "Command palette" },
  { action: "settings", title: "Settings" },
  { action: "rail", title: "Show or hide the rail" },
  { action: "split", title: "Transcript and changes side by side" },
  { action: "nextTab", title: "Next tab" },
  { action: "prevTab", title: "Previous tab" },
  { action: "closeTab", title: "Close tab" },
  { action: "attention", title: "Jump to the session that needs you" },
  { action: "railPrev", title: "Previous row in the rail" },
  { action: "railNext", title: "Next row in the rail" },
];

/** Push the appearance into the stylesheet. Tokens, not rules: everything
 *  downstream already reads these. */
export function applyAppearance(settings: AppSettings) {
  const root = document.documentElement;
  const { appearance, terminal } = settings;
  const set = (name: string, value: string | null) => {
    if (value === null || value === "") root.style.removeProperty(name);
    else root.style.setProperty(name, value);
  };
  set("--ui-font", appearance.uiFont.trim() === "" ? null : `${quoteFont(appearance.uiFont)}, system-ui, sans-serif`);
  set("--ui-scale", appearance.uiScale === 1 ? null : String(appearance.uiScale));
  set("--mono-font", terminal.font.trim() === "" ? null : `${quoteFont(terminal.font)}, monospace`);
  root.dataset.density = appearance.density === "compact" ? "compact" : "";
  // `system` follows the OS; the two named themes pin it. Read by the
  // tokens, which define both palettes and pick by this attribute.
  root.dataset.theme = appearance.theme === "light" || appearance.theme === "system" ? appearance.theme : "dark";
}

function quoteFont(name: string): string {
  const trimmed = name.trim();
  return /^["']/.test(trimmed) ? trimmed : `"${trimmed}"`;
}

export function Settings({
  initial,
  projects,
  available,
  onClose,
  onSaved,
  onError,
}: {
  initial: SettingsView;
  projects: ProjectView[];
  available: HostedView[];
  onClose: () => void;
  /** The view as Rust now holds it, after a save. */
  onSaved: (view: SettingsView) => void;
  onError: (message: string) => void;
}) {
  const [section, setSection] = useState<Section>("appearance");
  const [draft, setDraft] = useState<AppSettings>(initial.settings);
  const [model, setModel] = useState(initial.model);
  const [busy, setBusy] = useState(false);
  const dirty =
    JSON.stringify(draft) !== JSON.stringify(initial.settings) ||
    model.provider !== initial.model.provider ||
    model.name !== initial.model.name;

  // Live preview; the saved value is restored if the dialog is left.
  useEffect(() => {
    applyAppearance(draft);
  }, [draft]);
  useEffect(() => () => applyAppearance(initial.settings), [initial.settings]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const save = async () => {
    setBusy(true);
    try {
      let view = await api.saveSettings(draft);
      if (model.provider !== initial.model.provider || model.name !== initial.model.name) {
        view = await api.setDefaultModel(model);
      }
      onSaved(view);
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(false);
    }
  };

  const patch = <K extends "appearance" | "terminal">(key: K, value: Partial<AppSettings[K]>) =>
    setDraft((d) => ({ ...d, [key]: { ...d[key], ...value } }));

  return (
    <div className="scrim" onMouseDown={onClose}>
      <div className="settings" role="dialog" aria-label="Settings" onMouseDown={(e) => e.stopPropagation()}>
        <nav className="settings-nav">
          <h1>Settings</h1>
          {SECTIONS.map((s) => (
            <button key={s.id} className={section === s.id ? "on" : undefined} onClick={() => setSection(s.id)}>
              {s.title}
            </button>
          ))}
        </nav>

        <div className="settings-body">
          <header>
            <h2>{SECTIONS.find((s) => s.id === section)?.title}</h2>
            <button className="settings-close" onClick={onClose} aria-label="Close">
              <IconClose size={13} />
            </button>
          </header>

          <div className="settings-page">
            {section === "appearance" && (
              <>
                <Field label="Interface font" hint="Leave empty for the system face.">
                  <input
                    value={draft.appearance.uiFont}
                    placeholder="Segoe UI Variable Text, Inter, …"
                    onChange={(e) => patch("appearance", { uiFont: e.target.value })}
                  />
                </Field>
                <Field label="Interface size" hint={`${Math.round(draft.appearance.uiScale * 100)}% of the base sizes.`}>
                  <input
                    type="range"
                    min={0.85}
                    max={1.3}
                    step={0.05}
                    value={draft.appearance.uiScale}
                    onChange={(e) => patch("appearance", { uiScale: Number(e.target.value) })}
                  />
                </Field>
                <Field label="Theme">
                  <Segmented
                    value={draft.appearance.theme}
                    options={[
                      { value: "dark", title: "Dark" },
                      { value: "light", title: "Light" },
                      { value: "system", title: "System" },
                    ]}
                    onChange={(theme) => patch("appearance", { theme })}
                  />
                </Field>
                <Field label="Density" hint="Row heights and paddings; type is unchanged.">
                  <Segmented
                    value={draft.appearance.density}
                    options={[
                      { value: "comfortable", title: "Comfortable" },
                      { value: "compact", title: "Compact" },
                    ]}
                    onChange={(density) => patch("appearance", { density })}
                  />
                </Field>
              </>
            )}

            {section === "terminal" && (
              <>
                <Field label="Font" hint="A monospaced face. Nerd Font glyphs need one that has them.">
                  <input
                    value={draft.terminal.font}
                    placeholder="JetBrainsMono NFM, Cascadia Mono, …"
                    onChange={(e) => patch("terminal", { font: e.target.value })}
                  />
                </Field>
                <Field label="Size" hint="Pixels. Applies to terminals opened from now on.">
                  <input
                    type="number"
                    min={9}
                    max={24}
                    value={draft.terminal.fontSize}
                    onChange={(e) => patch("terminal", { fontSize: clamp(Number(e.target.value), 9, 24) })}
                  />
                </Field>
                <Field label="Line height" hint="1.0 keeps box-drawing glyphs contiguous; provider interfaces are full of them.">
                  <input
                    type="number"
                    min={0.9}
                    max={1.6}
                    step={0.05}
                    value={draft.terminal.lineHeight}
                    onChange={(e) => patch("terminal", { lineHeight: clamp(Number(e.target.value), 0.9, 1.6) })}
                  />
                </Field>
                <Field label="Scrollback" hint="Lines kept per terminal.">
                  <input
                    type="number"
                    min={1000}
                    max={100000}
                    step={1000}
                    value={draft.terminal.scrollback}
                    onChange={(e) => patch("terminal", { scrollback: clamp(Number(e.target.value), 1000, 100000) })}
                  />
                </Field>
                <Field label="On launch" hint="Bring every remembered terminal back running, its tool asked to continue. Off, they wait for Resume.">
                  <select
                    value={draft.terminal.resumeOnLaunch ? "resume" : "wait"}
                    onChange={(e) => patch("terminal", { resumeOnLaunch: e.target.value === "resume" })}
                  >
                    <option value="resume">Resume every terminal</option>
                    <option value="wait">List them ended, resume by hand</option>
                  </select>
                </Field>
                <Field label="Opens as" hint="How a terminal in its own tab is shown. Each terminal's header menu can change its own.">
                  <select value={draft.terminal.view} onChange={(e) => patch("terminal", { view: e.target.value })}>
                    <option value="card">Card overview</option>
                    <option value="tui">Terminal, with the card's header</option>
                    <option value="plain">Plain terminal</option>
                  </select>
                </Field>
              </>
            )}

            {section === "agents" && (
              <>
                <Field label="Open in editor" hint="Run with the path appended. Empty uses the platform's opener.">
                  <input
                    value={draft.editor}
                    placeholder="code"
                    onChange={(e) => setDraft((d) => ({ ...d, editor: e.target.value }))}
                  />
                </Field>
                <p className="settings-lead">
                  Each agent runs as itself in a terminal axio owns, in a worktree of its own. What is set here is
                  passed on its command line every time it is started.
                </p>
                {available.length === 0 && <p className="settings-lead">No agent tools were found on this machine.</p>}
                {available.map((a) => (
                  <Field
                    key={a.harness}
                    label={a.label}
                    hint={`Arguments for \`${a.harness}\`, split the way a shell would.`}
                    accent={`var(${a.accentVar})`}
                  >
                    <input
                      value={draft.agents[a.harness]?.args ?? ""}
                      placeholder="--flag value"
                      onChange={(e) =>
                        setDraft((d) => ({
                          ...d,
                          agents: { ...d.agents, [a.harness]: { args: e.target.value } },
                        }))
                      }
                    />
                  </Field>
                ))}
              </>
            )}

            {section === "model" && (
              <>
                <p className="settings-lead">
                  What axio's own sessions start on. Written to <code>{initial.configPath}</code>, the same file the
                  command line reads; everything else in it is left alone.
                </p>
                <Field label="Provider">
                  <select value={model.provider} onChange={(e) => setModel({ ...model, provider: e.target.value })}>
                    {initial.providers.map((p) => (
                      <option key={p} value={p}>
                        {p}
                      </option>
                    ))}
                    {!initial.providers.includes(model.provider) && <option value={model.provider}>{model.provider}</option>}
                  </select>
                </Field>
                <Field label="Model" hint="Not checked until the next request; a name the provider does not know fails there.">
                  <input value={model.name} onChange={(e) => setModel({ ...model, name: e.target.value })} />
                </Field>
              </>
            )}

            {section === "repositories" && (
              <>
                <p className="settings-lead">
                  Repositories under supervision. The list is the session index: a repository with no sessions is
                  forgotten on restart, so there is nothing here to delete.
                </p>
                {projects.length === 0 && <p className="settings-lead">None yet.</p>}
                <ul className="settings-list">
                  {projects.map((p) => (
                    <li key={p.id}>
                      <IconRepo size={13} />
                      <span className="title">{p.name}</span>
                      <span className="detail">{p.root}</span>
                      <span className="count">
                        {p.openSessions} open · {p.totalSessions} total
                      </span>
                    </li>
                  ))}
                </ul>
              </>
            )}

            {section === "keyboard" && (
              <ul className="settings-list">
                {ACTIONS.filter((a) => a.action in chords).map((a) => (
                  <li key={a.action}>
                    <span className="title">{a.title}</span>
                    <kbd>{label(a.action)}</kbd>
                  </li>
                ))}
                <li>
                  <span className="title">Tab by number</span>
                  <kbd>{label("palette").replace("K", "1")}–{label("palette").replace("K", "9")}</kbd>
                </li>
              </ul>
            )}
          </div>

          <footer>
            <span className="settings-path">{initial.appPath}</span>
            <button className="act" onClick={onClose} disabled={busy}>
              {dirty ? "Cancel" : "Close"}
            </button>
            <button className="act primary" onClick={() => void save()} disabled={busy || !dirty}>
              Save
            </button>
          </footer>
        </div>
      </div>
    </div>
  );
}

function clamp(n: number, lo: number, hi: number): number {
  if (!Number.isFinite(n)) return lo;
  return Math.min(hi, Math.max(lo, n));
}

function Field({
  label,
  hint,
  accent,
  children,
}: {
  label: string;
  hint?: string;
  accent?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="field" style={accent ? { ["--agent-accent" as string]: accent } : undefined}>
      <span className="field-label">
        {accent && <span className="dot" />}
        {label}
      </span>
      <span className="field-control">{children}</span>
      {hint && <span className="field-hint">{hint}</span>}
    </label>
  );
}

function Segmented({
  value,
  options,
  onChange,
}: {
  value: string;
  options: { value: string; title: string }[];
  onChange: (value: string) => void;
}) {
  return (
    <div className="views" role="radiogroup">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          className={value === o.value ? "view on" : "view"}
          onClick={() => onChange(o.value)}
        >
          {o.title}
        </button>
      ))}
    </div>
  );
}
