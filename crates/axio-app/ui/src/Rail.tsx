import { useEffect, useRef, useState } from "react";
import { accentFor, type HostedView, type SessionView, type Snapshot } from "./bridge";
import {
  hostedMenu as buildHostedMenu,
  projectMenu as buildProjectMenu,
  sessionMenu as buildSessionMenu,
  type MenuHooks,
} from "./menus";
import { RowMenu, type MenuItem } from "./RowMenu";
import { label } from "./shortcuts";
import { sameTab, type Tab } from "./Tabs";
import { IconHistory, IconMore, IconPlus, IconRepo, IconStart, IconTerminal } from "./icons";

// The left column: what exists, grouped by repository, and the other agents.
//
// Navigation, not a form. The composer lives in the pane; a row here opens a
// tab, and a row's colour and dot say what it is doing without opening it —
// which is the point of a rail when several agents are running. A word is
// added only where the dot cannot carry it: a question waiting, or a turn that
// finished while nobody was looking. Sessions started together sit under one
// group row, which opens them side by side.
//
// Every row has a menu — right-click, or the ⋯ that appears on hover — with
// the things a person does to a row rather than to the work: name it, find
// its directory, copy its branch, end it.
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
  onCloseSession,
  onChanged,
  onError,
  menuOpen = false,
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
  onStartTerminal: (harness: string, isolation: "worktree" | "direct") => void;
  onStopTerminal: (id: string) => void;
  onCloseSession: (id: string, discard: boolean) => void;
  /** Something the rail did changed the world; re-read it. */
  onChanged: () => void;
  onError: (message: string) => void;
  /** Start with the "new" menu open — for looking at it under the mock. */
  menuOpen?: boolean;
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
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);

  const hooks: MenuHooks = { onChanged, onError, onCloseSession, onStopTerminal };
  const sessionMenu = (s: SessionView) => buildSessionMenu(s, hooks);
  const hostedMenu = (h: HostedView) => buildHostedMenu(h, hooks);
  const projectMenu = (root: string) => buildProjectMenu(root, onError);

  const openMenu = (e: React.MouseEvent, items: MenuItem[]) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ items, at: { x: e.clientX, y: e.clientY } });
  };

  const sessionRow = (s: SessionView) => {
    const tab: Tab = { kind: "session", id: s.id };
    const needs = attention.has(s.id);
    const on = sameTab(active, tab);
    const fresh = unread.has(s.id) && !on;
    const chip = needs ? "needs you" : fresh ? "done" : !s.open ? "closed" : null;
    return (
      <div className="row-host" key={s.id} onContextMenu={(e) => openMenu(e, sessionMenu(s))}>
        <button
          className={`session${on ? " active" : ""}${s.open ? "" : " done"}${needs ? " needs" : ""}${fresh ? " fresh" : ""}`}
          style={{ ["--agent-accent" as string]: accentFor(s) }}
          onClick={() => onOpen(tab)}
          title={`${s.title ?? s.label ?? s.shortId} — ${needs ? "needs you" : s.open ? s.status : "closed"}`}
        >
          <span className={`dot ${needs ? "needs" : s.open ? s.status : "closed"}`} />
          <span className="label">{s.title ?? s.label ?? "(no prompt)"}</span>
          {chip && <span className={`chip ${chip.replace(" ", "-")}`}>{chip}</span>}
        </button>
        <button className="row-more" aria-label="More" onClick={(e) => openMenu(e, sessionMenu(s))}>
          <IconMore size={12} />
        </button>
      </div>
    );
  };

  const hostedRow = (h: HostedView) => {
    const tab: Tab = { kind: "terminal", id: h.id };
    return (
      <div className="row-host" key={h.id} onContextMenu={(e) => openMenu(e, hostedMenu(h))}>
        <button
          className={`session${sameTab(active, tab) ? " active" : ""}`}
          style={{ ["--agent-accent" as string]: `var(${h.accentVar})` }}
          onClick={() => onOpen(tab)}
          title={h.cwd}
        >
          <span className={`dot ${h.status === "running" ? "running" : "closed"}`} />
          <span className="label">
            {h.name}
            <em>{h.branch ?? h.cwd.split(/[\\/]/).filter(Boolean).pop() ?? ""}</em>
          </span>
          {h.status !== "running" && <small>{h.status}</small>}
        </button>
        <button className="row-more" aria-label="More" onClick={(e) => openMenu(e, hostedMenu(h))}>
          <IconMore size={12} />
        </button>
      </div>
    );
  };

  return (
    <nav className="rail">
      {/* Two ways to add something, side by side: a place to work, and work.
          "New" is a menu because the work can be an axio session or another
          agent's tool in a terminal, and a row of launcher chips said the same
          thing with more chrome. */}
      <div className="rail-add">
        <NewMenu
          composing={active === null}
          available={available}
          onNew={onNew}
          onStartTerminal={onStartTerminal}
          defaultOpen={menuOpen}
        />
        <button className="rail-new" onClick={onAddRepository} title={`Add a repository  ${label("addRepository")}`}>
          <IconPlus size={13} />
          Repository
        </button>
      </div>

      <div className="rail-head">
        <span>Repositories</span>
        <span className="rail-tools">
          <button
            className={history ? "rail-tool on" : "rail-tool"}
            onClick={() => setHistory((h) => !h)}
            title={history ? "Hide closed sessions" : `Show closed sessions${hidden > 0 ? ` (${hidden})` : ""}`}
            aria-pressed={history}
            aria-label="Show closed sessions"
          >
            <IconHistory size={13} />
          </button>
        </span>
      </div>

      <div className="rail-list">
        {projects.map((project) => {
          const mine = sessions.filter((s) => s.projectId === project.id);
          const groups = [...new Set(mine.map((s) => s.group).filter((g): g is string => g !== null))];
          const loose = mine.filter((s) => s.group === null);
          return (
            <section className="project" key={project.id}>
              <h2 title={project.root} onContextMenu={(e) => openMenu(e, projectMenu(project.root))}>
                <IconRepo size={13} />
                <span className="name">{project.name}</span>
                <span className="count">{project.openSessions}</span>
              </h2>
              <div className="sessions">
                {groups.map((g) => {
                  const members = mine.filter((s) => s.group === g);
                  const terminals = hosted.filter((h) => h.group === g);
                  const tab: Tab = { kind: "group", id: g };
                  const on = sameTab(active, tab);
                  const needs = members.some((s) => attention.has(s.id));
                  const working =
                    members.some((s) => s.status === "running") || terminals.some((h) => h.status === "running");
                  const size = members.length + terminals.length;
                  return (
                    <div className="group" key={g}>
                      <button
                        className={`session group-row${on ? " active" : ""}${needs ? " needs" : ""}`}
                        onClick={() => onOpen(tab)}
                        title={`${size} agents on one prompt — open side by side`}
                      >
                        <span className={`dot ${needs ? "needs" : working ? "running" : "idle"}`} />
                        <span className="label">{members[0]?.title ?? members[0]?.label ?? "group"}</span>
                        <span className="chip">{size} agent{size === 1 ? "" : "s"}</span>
                      </button>
                      <div className="group-members">
                        {members.map(sessionRow)}
                        {terminals.map(hostedRow)}
                      </div>
                    </div>
                  );
                })}
                {loose.map(sessionRow)}
              </div>
            </section>
          );
        })}
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
          its own history and its own idea of what a session is. Those in a
          group are listed under it above, not here twice. */}
      <div className="hosted">
        <div className="rail-head">
          <span>Terminals</span>
          {hosted.length > 0 && <span className="count">{hosted.length}</span>}
        </div>
        {hosted.filter((h) => h.group === null).map(hostedRow)}
        {hosted.length === 0 && (
          <p className="rail-empty">
            None running. <b>New</b> above starts one.
          </p>
        )}
      </div>

      {menu && <RowMenu items={menu.items} at={menu.at} onClose={() => setMenu(null)} />}
    </nav>
  );
}

// The "new" button and what drops out of it.
//
// The first entry is the composer — an axio session in its own worktree, or a
// group of them — and the rest are the agents this machine can host, one
// terminal each. The menu lists only what can actually start: a harness whose
// executable is not on this machine is not offered, rather than offered and
// failed.
function NewMenu({
  composing,
  available,
  onNew,
  onStartTerminal,
  defaultOpen,
}: {
  composing: boolean;
  available: HostedView[];
  onNew: () => void;
  onStartTerminal: (harness: string, isolation: "worktree" | "direct") => void;
  defaultOpen: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const host = useRef<HTMLDivElement>(null);

  // Outside click or Escape closes it; nothing else does but choosing.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (host.current && !host.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="new-menu" ref={host}>
      <button
        className={`rail-new primary${composing ? " on" : ""}`}
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        title={`New session ${label("newSession")} · terminal ${label("newTerminal")}`}
      >
        <IconPlus size={13} />
        New
      </button>
      {open && (
        <ul className="menu" role="menu">
          <li role="none">
            <button
              role="menuitem"
              onClick={() => {
                setOpen(false);
                onNew();
              }}
            >
              <IconStart size={13} />
              <span className="menu-title">Session, or a group</span>
              <span className="menu-detail">axio, its own worktree · {label("newSession")}</span>
            </button>
          </li>
          {available.length > 0 && <li className="menu-rule" role="separator" />}
          {/* Each agent gets a worktree of its own, like a session. Holding
              Alt runs it in the checkout instead — a choice, never a fallback. */}
          {available.map((a, n) => (
            <li role="none" key={a.harness} style={{ ["--agent-accent" as string]: `var(${a.accentVar})` }}>
              <button
                role="menuitem"
                title={`${a.label} in a terminal, in its own worktree. Alt-click for the checkout itself.`}
                onClick={(e) => {
                  setOpen(false);
                  onStartTerminal(a.harness, e.altKey ? "direct" : "worktree");
                }}
              >
                <IconTerminal size={13} />
                <span className="menu-title">{a.label}</span>
                <span className="menu-detail">
                  in a terminal, its own worktree{n === 0 ? ` · ${label("newTerminal")}` : ""}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
