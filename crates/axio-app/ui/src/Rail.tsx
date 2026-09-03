import { useState } from "react";
import { accentFor, type HostedView, type Snapshot } from "./bridge";
import { sameTab, type Tab } from "./Tabs";
import { IconHistory, IconPlus, IconRepo, IconStop, IconTerminal } from "./icons";

// The left column: what exists, grouped by repository, and the other agents.
//
// Navigation, not a form. The composer lives in the pane; a row here opens a
// tab, and a row's colour, dot and chip say what it is doing without opening
// it — which is the point of a rail when several agents are running.
export function Rail({
  snapshot,
  active,
  attention,
  unread,
  onAddRepository,
  onNew,
  onOpen,
  hosted,
  available,
  onStartTerminal,
  onStopTerminal,
}: {
  snapshot: Snapshot | null;
  active: Tab | null;
  attention: Set<string>;
  unread: Set<string>;
  onAddRepository: () => void;
  onNew: () => void;
  onOpen: (tab: Tab) => void;
  hosted: HostedView[];
  available: HostedView[];
  onStartTerminal: (harness: string) => void;
  onStopTerminal: (id: string) => void;
}) {
  const projects = snapshot?.projects ?? [];
  // Closed sessions are history. They stay reachable — a kept worktree is
  // still worth reading — but a rail that lists every session ever started
  // buries the three that are running under the forty that are not.
  const [history, setHistory] = useState(false);
  const sessions = (snapshot?.sessions ?? []).filter(
    (s) => history || s.open || sameTab(active, { kind: "session", id: s.id }),
  );
  const hidden = (snapshot?.sessions.length ?? 0) - sessions.length;

  return (
    <nav className="rail">
      <button className={active === null ? "rail-new on" : "rail-new"} onClick={onNew}>
        <IconPlus size={13} />
        New session
      </button>

      <div className="rail-head">
        <span>Repositories</span>
        <span className="rail-tools">
          <span className="count">{projects.length}</span>
          <button
            className={history ? "rail-tool on" : "rail-tool"}
            onClick={() => setHistory((h) => !h)}
            title={history ? "Hide closed sessions" : `Show closed sessions${hidden > 0 ? ` (${hidden})` : ""}`}
            aria-pressed={history}
            aria-label="Show closed sessions"
          >
            <IconHistory size={13} />
          </button>
          <button className="rail-tool" onClick={onAddRepository} title="Add a repository" aria-label="Add a repository">
            <IconPlus size={13} />
          </button>
        </span>
      </div>

      <div className="rail-list">
        {projects.map((project) => (
          <section className="project" key={project.id}>
            <h2 title={project.root}>
              <IconRepo size={13} />
              <span className="name">{project.name}</span>
              <span className="count">{project.openSessions}</span>
            </h2>
            <div className="sessions">
              {sessions
                .filter((s) => s.projectId === project.id)
                .map((s) => {
                  const tab: Tab = { kind: "session", id: s.id };
                  const needs = attention.has(s.id);
                  const on = sameTab(active, tab);
                  const chip = needs
                    ? "needs you"
                    : !s.open
                      ? "closed"
                      : s.status === "running"
                        ? "working"
                        : s.status === "idle"
                          ? unread.has(s.id)
                            ? "done"
                            : "idle"
                          : "elsewhere";
                  return (
                    <button
                      key={s.id}
                      className={`session${on ? " active" : ""}${s.open ? "" : " done"}${needs ? " needs" : ""}${unread.has(s.id) && !on ? " fresh" : ""}`}
                      style={{ ["--agent-accent" as string]: accentFor(s) }}
                      onClick={() => onOpen(tab)}
                      title={s.workspace}
                    >
                      <span className={`dot ${needs ? "needs" : s.open ? s.status : "closed"}`} />
                      <span className="label">{s.label ?? "(no prompt)"}</span>
                      <span className={`chip ${chip.replace(" ", "-")}`}>{chip}</span>
                    </button>
                  );
                })}
            </div>
          </section>
        ))}
        {projects.length === 0 && !snapshot?.unavailable && (
          <p className="rail-empty">
            No repositories yet.{" "}
            <button className="link" onClick={onAddRepository}>
              Add one
            </button>
            .
          </p>
        )}
      </div>

      {/* Other agents' own tools, each in a terminal axio owns. Listed apart
          from supervised sessions on purpose: a hosted Claude Code is not an
          axio session with a different colour — it has its own approvals,
          its own history and its own idea of what a session is. */}
      <div className="hosted">
        <div className="rail-head">
          <span>Terminals</span>
          <span className="count">{hosted.length}</span>
        </div>
        <div className="hosted-launch">
          {available.map((a) => (
            <button
              key={a.harness}
              style={{ ["--agent-accent" as string]: `var(${a.accentVar})` }}
              onClick={() => onStartTerminal(a.harness)}
              title={`Run ${a.label} in a terminal`}
            >
              <IconTerminal size={12} />
              {a.label}
            </button>
          ))}
        </div>
        {hosted.map((h) => {
          const tab: Tab = { kind: "terminal", id: h.id };
          return (
            <div className="hosted-row" key={h.id}>
              <button
                className={`session${sameTab(active, tab) ? " active" : ""}`}
                style={{ ["--agent-accent" as string]: `var(${h.accentVar})` }}
                onClick={() => onOpen(tab)}
                title={h.cwd}
              >
                <span className={`dot ${h.status === "running" ? "running" : "closed"}`} />
                <span className="label">
                  {h.label}
                  <em>{h.cwd.split(/[\\/]/).filter(Boolean).pop() ?? ""}</em>
                </span>
                <small>{h.status === "running" ? "live" : h.status}</small>
              </button>
              <button
                className="hosted-close"
                onClick={() => onStopTerminal(h.id)}
                title={`Stop ${h.label} and everything it started`}
                aria-label={`Stop ${h.label}`}
              >
                <IconStop size={12} />
              </button>
            </div>
          );
        })}
      </div>

      <div className="rail-foot">isolated worktrees · one branch each</div>
    </nav>
  );
}
