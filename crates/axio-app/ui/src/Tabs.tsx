import type { HostedView, SessionView } from "./bridge";
import { IconClose, IconPlus } from "./icons";

// What is open, across the top of the pane.
//
// A tab is a *place the window is looking*, not a session: closing one hides
// it and stops nothing. Sessions and hosted terminals share the strip because
// they share the pane, and a person switching between an agent's terminal and
// their own session's diff should not have to remember which list it is in.
export type Tab = { kind: "session"; id: string } | { kind: "terminal"; id: string };

export function tabKey(tab: Tab): string {
  return `${tab.kind}:${tab.id}`;
}

export function sameTab(a: Tab | null, b: Tab | null): boolean {
  return a !== null && b !== null && a.kind === b.kind && a.id === b.id;
}

export function Tabs({
  tabs,
  active,
  sessions,
  hosted,
  attention,
  unread,
  onActivate,
  onClose,
  onNew,
}: {
  tabs: Tab[];
  active: Tab | null;
  sessions: SessionView[];
  hosted: HostedView[];
  /** Session ids with a question waiting. */
  attention: Set<string>;
  /** Session ids that finished a turn while not being looked at. */
  unread: Set<string>;
  onActivate: (tab: Tab) => void;
  onClose: (tab: Tab) => void;
  onNew: () => void;
}) {
  return (
    <div className="tabs-strip" role="tablist">
      {tabs.map((tab, n) => {
        const on = sameTab(tab, active);
        let title = "";
        let sub = "";
        let dot = "closed";
        let accent = "var(--accent)";
        if (tab.kind === "session") {
          const s = sessions.find((x) => x.id === tab.id);
          title = s?.label ?? s?.shortId ?? tab.id;
          sub = s?.projectName ?? "";
          dot = s ? (s.open ? s.status : "closed") : "closed";
          accent = s?.isolation === "direct" ? "var(--agent-pi)" : "var(--agent-axio)";
        } else {
          const h = hosted.find((x) => x.id === tab.id);
          title = h?.label ?? tab.id;
          sub = h?.cwd.split(/[\\/]/).filter(Boolean).pop() ?? "";
          dot = h?.status === "running" ? "running" : "closed";
          accent = h ? `var(${h.accentVar})` : accent;
        }
        const needs = tab.kind === "session" && attention.has(tab.id);
        const fresh = tab.kind === "session" && unread.has(tab.id) && !on;
        return (
          <div
            key={tabKey(tab)}
            role="tab"
            aria-selected={on}
            tabIndex={0}
            className={`tab-item${on ? " on" : ""}${needs ? " needs" : ""}${fresh ? " fresh" : ""}`}
            style={{ ["--agent-accent" as string]: accent }}
            title={`${title}${sub ? ` — ${sub}` : ""}  (${n < 9 ? `⌘${n + 1}` : ""})`}
            onClick={() => onActivate(tab)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") onActivate(tab);
            }}
            onAuxClick={(e) => {
              if (e.button === 1) onClose(tab);
            }}
          >
            <span className={`dot ${needs ? "needs" : dot}`} />
            <span className="tab-title">{title}</span>
            {sub && <span className="tab-sub">{sub}</span>}
            <button
              className="tab-close"
              aria-label={`Close tab ${title}`}
              onClick={(e) => {
                e.stopPropagation();
                onClose(tab);
              }}
            >
              <IconClose size={11} />
            </button>
          </div>
        );
      })}
      <button className="tab-new" onClick={onNew} title="New session" aria-label="New session">
        <IconPlus size={13} />
      </button>
    </div>
  );
}
