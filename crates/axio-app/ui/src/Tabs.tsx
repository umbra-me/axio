import type { ReactNode } from "react";
import type { HostedView, SessionView, Snapshot } from "./bridge";
import { IconClose } from "./icons";

// What is open, across the top of the pane.
//
// A tab is a *place the window is looking*, not a session: closing one hides
// it and stops nothing. Sessions and hosted terminals share the strip because
// they share the pane, and a person switching between an agent's terminal and
// their own session's diff should not have to remember which list it is in.
//
// The strip is also the pane's toolbar. Whatever the front tab can be told to
// do — show its changes, close its worktree, stop its terminal — arrives as
// `tail` and sits at the right end of the same row, so a session costs one bar
// of chrome rather than two saying the same name. With nothing open there is
// nothing to look at and the strip is not drawn; the rail is how a tab opens.
export type Tab =
  | { kind: "session"; id: string }
  | { kind: "terminal"; id: string }
  /** Several members side by side; `id` is the group id they share. */
  | { kind: "group"; id: string }
  /** Everything running in one repository, side by side; `id` is the project id. */
  | { kind: "project"; id: string };

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
  projects,
  attention,
  unread,
  onActivate,
  onClose,
  tail,
}: {
  tabs: Tab[];
  active: Tab | null;
  sessions: SessionView[];
  hosted: HostedView[];
  projects: Snapshot["projects"];
  /** Session ids with a question waiting. */
  attention: Set<string>;
  /** Session ids that finished a turn while not being looked at. */
  unread: Set<string>;
  onActivate: (tab: Tab) => void;
  onClose: (tab: Tab) => void;
  /** The front tab's controls, at the right end of the strip. */
  tail?: ReactNode;
}) {
  if (tabs.length === 0) return null;
  return (
    <div className="topbar">
      <div className="tabs-strip" role="tablist">
        {tabs.map((tab, n) => {
          const on = sameTab(tab, active);
          let title = "";
          let sub = "";
          let dot = "closed";
          let accent = "var(--accent)";
          if (tab.kind === "session") {
            const s = sessions.find((x) => x.id === tab.id);
            title = s?.title ?? s?.label ?? s?.shortId ?? tab.id;
            sub = s?.projectName ?? "";
            dot = s ? (s.open ? s.status : "closed") : "closed";
            accent = s?.isolation === "direct" ? "var(--agent-pi)" : "var(--agent-axio)";
          } else if (tab.kind === "project") {
            const p = projects.find((x) => x.id === tab.id);
            const members = [
              ...sessions.filter((s) => s.projectId === tab.id && s.open),
              ...hosted.filter((h) => p !== undefined && h.repo === p.root),
            ];
            title = p?.name ?? tab.id;
            sub = `${members.length} agent${members.length === 1 ? "" : "s"}`;
            dot = members.some((m) => m.status === "running") ? "running" : "idle";
          } else if (tab.kind === "group") {
            const members = [
              ...sessions.filter((s) => s.group === tab.id),
              ...hosted.filter((h) => h.group === tab.id),
            ];
            const first = sessions.find((s) => s.group === tab.id);
            title = first?.title ?? first?.label ?? "group";
            sub = `${members.length} agent${members.length === 1 ? "" : "s"}`;
            dot = members.some((m) => m.status === "running") ? "running" : "idle";
          } else {
            const h = hosted.find((x) => x.id === tab.id);
            title = h?.name ?? tab.id;
            sub = h?.branch ?? h?.cwd.split(/[\\/]/).filter(Boolean).pop() ?? "";
            dot = h?.status === "running" ? "running" : "closed";
            accent = h ? `var(${h.accentVar})` : accent;
          }
          const needs = (tab.kind === "session" || tab.kind === "terminal") && attention.has(tab.id);
          const fresh = (tab.kind === "session" || tab.kind === "terminal") && unread.has(tab.id) && !on;
          return (
            <div
              key={tabKey(tab)}
              role="tab"
              aria-selected={on}
              tabIndex={0}
              className={`tab-item${on ? " on" : ""}${needs ? " needs" : ""}${fresh ? " fresh" : ""}`}
              style={{ ["--agent-accent" as string]: accent }}
              title={`${title}${sub ? ` — ${sub}` : ""}${n < 9 ? `  (⌘${n + 1})` : ""}`}
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
      </div>
      {tail && <div className="topbar-tail">{tail}</div>}
    </div>
  );
}
