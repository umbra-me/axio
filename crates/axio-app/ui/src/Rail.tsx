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
import { IconHistory, IconMore, IconPlus, IconRepo, IconStart, IconTerminal, IconChevron } from "./icons";

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
  onResumeTerminal,
  onRemoveTerminal,
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
  /** Go to the composer, for this repository when one is named. */
  onNew: (root?: string) => void;
  onOpen: (tab: Tab) => void;
  hosted: HostedView[];
  available: HostedView[];
  onStartTerminal: (harness: string, isolation: "worktree" | "direct", root?: string) => void;
  onStopTerminal: (id: string) => void;
  onResumeTerminal: (id: string) => void;
  onRemoveTerminal: (id: string) => void;
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
  // Folded repositories, by project id. A fold is a way of looking, kept by
  // the window and not by anything the supervisor knows.
  const [folded, setFolded] = useState<Set<string>>(new Set());
  const toggleFold = (id: string) =>
    setFolded((f) => {
      const next = new Set(f);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const sessions = (snapshot?.sessions ?? []).filter(
    (s) => history || s.open || sameTab(active, { kind: "session", id: s.id }),
  );
  const hidden = (snapshot?.sessions.length ?? 0) - sessions.length;
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);

  const hooks: MenuHooks = { onChanged, onError, onCloseSession, onStopTerminal, onResumeTerminal, onRemoveTerminal };
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
    const on = sameTab(active, tab);
    const needs = attention.has(h.id);
    const fresh = unread.has(h.id) && !on;
    // The agent's own word first, the quiet-timer guess only for a tool
    // that has none.
    const chip = needs ? "needs you" : h.agentStatus === "done" && !on ? "done" : fresh ? "new output" : null;
    return (
      <div className="row-host" key={h.id} onContextMenu={(e) => openMenu(e, hostedMenu(h))}>
        <button
          className={`session${on ? " active" : ""}${fresh ? " fresh" : ""}${needs ? " needs" : ""}`}
          style={{ ["--agent-accent" as string]: `var(${h.accentVar})` }}
          onClick={() => onOpen(tab)}
          title={h.cwd}
        >
          <span className={`dot ${needs ? "needs" : h.status === "running" ? (h.agentStatus === "working" ? "running" : "idle") : "closed"}`} />
          <span className="label">
            {h.name}
            <em>{h.branch ?? h.cwd.split(/[\\/]/).filter(Boolean).pop() ?? ""}</em>
          </span>
          {chip ? (
            <span className={`chip ${needs ? "needs-you" : "done"}`}>{chip}</span>
          ) : (
            h.status !== "running" && <small>{h.stopped ? "stopped" : h.status}</small>
          )}
        </button>
        <button className="row-more" aria-label="More" onClick={(e) => openMenu(e, hostedMenu(h))}>
          <IconMore size={12} />
        </button>
      </div>
    );
  };

  return (
    <nav className="rail">
      {/* Adding goes where the thing is added: a repository from the heading
          of the list of them, work from the heading of the repository it is
          for. Nothing above the list says "New" — the list is the place. */}
      <div className="rail-head">
        <span>Repositories</span>
        <span className="rail-tools">
          <button className="rail-tool" onClick={onAddRepository} title={`Add a repository  ${label("addRepository")}`} aria-label="Add a repository">
            <IconPlus size={13} />
          </button>
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
          // A repository holds every agent working in it, whichever kind:
          // axio sessions, other agents' terminals, and the groups either
          // started in. A terminal is a thread under its repository, not a
          // pile of its own at the bottom.
          const mine = sessions.filter((s) => s.projectId === project.id);
          const terms = hosted.filter((h) => h.repo === project.root);
          const groups = [
            ...new Set([...mine.map((s) => s.group), ...terms.map((h) => h.group)].filter((g): g is string => g !== null)),
          ];
          const loose = mine.filter((s) => s.group === null);
          const looseTerms = terms.filter((h) => h.group === null);
          const live = project.openSessions + terms.filter((h) => h.status === "running").length;
          const tab: Tab = { kind: "project", id: project.id };
          const isFolded = folded.has(project.id);
          return (
            <section className={`project${isFolded ? " folded" : ""}`} key={project.id}>
              <h2
                title={project.root}
                onContextMenu={(e) =>
                  openMenu(e, [
                    { kind: "action", title: "Open side by side", run: () => onOpen(tab) },
                    { kind: "rule" },
                    ...projectMenu(project.root),
                  ])
                }
              >
                <button
                  className="fold"
                  aria-label={isFolded ? `Unfold ${project.name}` : `Fold ${project.name}`}
                  aria-expanded={!isFolded}
                  onClick={() => toggleFold(project.id)}
                >
                  <IconChevron size={11} />
                </button>
                {/* The name folds too: a heading that opens a pane when you
                    meant to tidy the list is a heading that moves the view
                    under you. Side by side is on the menu and in the palette. */}
                <button
                  className={`project-open${sameTab(active, tab) ? " active" : ""}`}
                  title={project.root}
                  onClick={() => toggleFold(project.id)}
                >
                  <IconRepo size={13} />
                  <span className="name">{project.name}</span>
                </button>
                <span className="count">{live}</span>
                <NewMenu
                  root={project.root}
                  available={available}
                  onNew={onNew}
                  onStartTerminal={onStartTerminal}
                  defaultOpen={menuOpen && project === projects[0]}
                />
              </h2>
              <div className="sessions">
                {groups.map((g) => {
                  const members = mine.filter((s) => s.group === g);
                  const terminals = terms.filter((h) => h.group === g);
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
                {looseTerms.map(hostedRow)}
                {mine.length === 0 && terms.length === 0 && (
                  <p className="rail-empty quiet-row">Nothing here yet.</p>
                )}
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

      {/* Terminals whose directory belongs to no repository listed above —
          started directly somewhere else. Every other terminal is a thread
          under its repository, with the sessions. */}
      {(() => {
        const elsewhere = hosted.filter((h) => !projects.some((p) => p.root === h.repo));
        return elsewhere.length > 0 ? (
          <div className="hosted">
            <div className="rail-head">
              <span>Elsewhere</span>
              <span className="count">{elsewhere.length}</span>
            </div>
            {elsewhere.map(hostedRow)}
          </div>
        ) : null;
      })()}

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
  root,
  available,
  onNew,
  onStartTerminal,
  defaultOpen,
}: {
  /** The repository the menu starts things in. */
  root: string;
  available: HostedView[];
  onNew: (root: string) => void;
  onStartTerminal: (harness: string, isolation: "worktree" | "direct", root: string) => void;
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
        className={`rail-tool${open ? " on" : ""}`}
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="New session or terminal here"
        title={`New here — session ${label("newSession")} · terminal ${label("newTerminal")}`}
      >
        <IconPlus size={13} />
      </button>
      {open && (
        <ul className="menu right" role="menu">
          <li role="none">
            <button
              role="menuitem"
              onClick={() => {
                setOpen(false);
                onNew(root);
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
                  onStartTerminal(a.harness, e.altKey ? "direct" : "worktree", root);
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
