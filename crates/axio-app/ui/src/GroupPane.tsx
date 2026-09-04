import { useCallback, useEffect, useRef, useState } from "react";
import { Approvals } from "./Approvals";
import {
  api,
  describe,
  listen,
  type ApprovalView,
  type HostedView,
  type SessionView,
  type SlashCommand,
  type TerminalSettings,
  type TranscriptView,
} from "./bridge";
import { Diff } from "./Diff";
import type { Tab } from "./Tabs";
import { hostedMenu, sessionMenu, type MenuHooks } from "./menus";
import { RowMenu, type MenuItem } from "./RowMenu";
import { HostedTerminal } from "./Terminal";
import { HostedChat } from "./TerminalPane";
import { Transcript } from "./Transcript";
import { balanced, place, reconcile, withRatio, type LayoutNode, type Side } from "./layout";
import {
  IconBranch,
  IconClose,
  IconDiff,
  IconExpand,
  IconGrid,
  IconListen,
  IconMessage,
  IconMore,
  IconPlus,
  IconResize,
  IconSplit,
  IconStart,
  IconStop,
  IconTerminal,
} from "./icons";

/** How big a card is. A preset name, or exact pixels from a drag. Chosen per
 *  member and kept by the window. */
export type CardSize = "small" | "wide" | "large" | "full" | { w: number; h: number };
const SIZES: { size: Exclude<CardSize, object>; title: string }[] = [
  { size: "small", title: "Small" },
  { size: "wide", title: "Wide" },
  { size: "large", title: "Large" },
  { size: "full", title: "Full width" },
];
const NEXT: Record<Exclude<CardSize, object>, Exclude<CardSize, object>> = {
  small: "wide",
  wide: "large",
  large: "full",
  full: "small",
};
const MIN_W = 240;
const MIN_H = 160;

// Several agents side by side: a group started on one prompt, or everything
// running in one repository.
//
// Each member is a card: who it is, what it is doing, and — live — what it is
// saying: a session's transcript as it streams, a hosted agent's real terminal
// at a smaller size. Changes swaps every card to its diff, read when the pane
// opens and again whenever a member's status changes — a `git diff` per token
// would be the wrong kind of live. A card's header opens the member's own tab;
// the pane is a way of looking, not a thing to drive.
type Mode = "overview" | "live" | "changes";

/** How one terminal is shown. `card` is the summary, `tui` the real terminal
 *  under the card's header, `plain` the terminal with no chrome at all —
 *  which only a tab of its own can offer; inside a group it reads as `tui`. */
export type TerminalView = "card" | "tui" | "plain" | "chat";

/** The menu entries that pick a terminal's view, with the current one marked. */
export function viewItems(current: TerminalView, choices: TerminalView[], onView: (view: TerminalView) => void): MenuItem[] {
  const titles: Record<TerminalView, string> = {
    card: "Card overview",
    tui: "Terminal, with this header",
    plain: "Plain terminal",
    chat: "Its transcript, as a chat",
  };
  return choices.map((v) => ({
    kind: "action" as const,
    title: `${v === current ? "● " : "○ "}${titles[v]}`,
    run: () => onView(v),
  }));
}

/** What the pane shows side by side. A group's members share a prompt and an
 *  id; a project's are whatever is open in that repository, session or
 *  terminal, grouped or not. */
export type Scope = { kind: "group"; id: string } | { kind: "project"; id: string; name: string };

export function GroupPane({
  scope,
  sessions,
  hosted,
  attention,
  approvals,
  terminal,
  available,
  projectRoot,
  onOpen,
  onChanged,
  onError,
  onCloseSession,
  onStopTerminal,
  onResumeTerminal,
  onRemoveTerminal,
  sizes,
  onSize,
  views,
  onView,
  layout,
  onLayout,
}: {
  scope: Scope;
  sessions: SessionView[];
  hosted: HostedView[];
  attention: Set<string>;
  /** Every question waiting, so a card can show its member's. */
  approvals: ApprovalView[];
  terminal: TerminalSettings | null;
  /** Agents this machine can host, for the Add menu. */
  available: HostedView[];
  /** The repository the members work on. */
  projectRoot: string | null;
  onOpen: (tab: Tab) => void;
  onChanged: () => void;
  onError: (message: string) => void;
  onCloseSession: (id: string, discard: boolean) => void;
  onStopTerminal: (id: string) => void;
  onResumeTerminal: (id: string) => void;
  onRemoveTerminal: (id: string) => void;
  /** Card sizes by member id. Held by the window, so they survive a tab switch. */
  sizes: Record<string, CardSize>;
  onSize: (id: string, size: CardSize) => void;
  /** Each terminal's own view, overriding the pane's Overview / Live for
   *  that card. Held by the window, like the sizes. */
  views: Record<string, TerminalView>;
  onView: (id: string, view: TerminalView) => void;
  /** Where the members sit. `null` is the grid. */
  layout: LayoutNode | null;
  onLayout: (node: LayoutNode | null) => void;
}) {
  const [mode, setMode] = useState<Mode>("overview");
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);
  const hooks: MenuHooks = { onChanged, onError, onCloseSession, onStopTerminal, onResumeTerminal, onRemoveTerminal };
  const openMenu = (e: React.MouseEvent, items: MenuItem[]) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ items, at: { x: e.clientX, y: e.clientY } });
  };
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState(false);
  const addHost = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!adding) return;
    const onDown = (e: MouseEvent) => {
      if (addHost.current && !addHost.current.contains(e.target as Node)) setAdding(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [adding]);
  const root = projectRoot;
  const mine = (s: SessionView) => (scope.kind === "group" ? s.group === scope.id : s.projectId === scope.id && s.open);
  const hostedMine = (h: HostedView) => (scope.kind === "group" ? h.group === scope.id : root !== null && h.repo === root);
  const members = [
    ...sessions.filter(mine).map((s) => ({ kind: "session" as const, s })),
    ...hosted.filter(hostedMine).map((h) => ({ kind: "terminal" as const, h })),
  ];
  const first = sessions.find(mine);
  const prompt = scope.kind === "group" ? (first?.label ?? "group") : scope.name;
  const key = `${scope.kind}:${scope.id}`;

  // One more member. In a group it joins the group and is asked the same
  // prompt; in a repository it is simply one more agent working there, in a
  // worktree of its own, waiting to be typed at.
  const add = async (harness: string | null) => {
    setAdding(false);
    if (!root) {
      onError("this repository is not known, so nothing can start in it");
      return;
    }
    setBusy(true);
    try {
      if (scope.kind === "group") {
        await api.addToGroup({ group: scope.id, path: root, prompt: first?.label ?? null, harness });
      } else if (harness === null) {
        await api.startSession({ path: root, prompt: null, isolation: null, group: null });
      } else {
        await api.hostedStart(harness, root, "worktree");
      }
      onChanged();
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(false);
    }
  };
  const statuses = members.map((m) => (m.kind === "session" ? m.s.status : m.h.status)).join(",");
  const ids = members.map((m) => (m.kind === "session" ? m.s.id : m.h.id));
  const idsKey = ids.join(",");

  // Members come and go; the layout follows. A member that left is taken
  // out of the tree, one that arrived is added on the right.
  useEffect(() => {
    if (!layout) return;
    const next = reconcile(layout, ids);
    if (JSON.stringify(next) !== JSON.stringify(layout)) onLayout(next);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idsKey]);

  // The master box reaches every member that has not been muted. Muted
  // rather than listening so a newcomer hears it without being asked.
  const [muted, setMuted] = useState<Set<string>>(new Set());
  const [master, setMaster] = useState("");
  const [masterBusy, setMasterBusy] = useState(false);
  const toggleMute = (id: string) =>
    setMuted((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const broadcast = async () => {
    const text = master.trim();
    if (!text) return;
    setMasterBusy(true);
    try {
      await Promise.all(
        members
          .filter((m) => !muted.has(m.kind === "session" ? m.s.id : m.h.id))
          .map((m) => (m.kind === "session" ? api.sendPrompt(m.s.id, text) : api.hostedSubmit(m.h.id, text))),
      );
      setMaster("");
      onChanged();
    } catch (e) {
      onError(describe(e));
    } finally {
      setMasterBusy(false);
    }
  };

  // Dragging a card's header, in the split layout, onto another card's
  // edge splits that card; onto its middle swaps the two.
  const [dragging, setDragging] = useState<string | null>(null);
  const [hover, setHover] = useState<{ id: string; side: Side } | null>(null);

  const [diffs, setDiffs] = useState<Record<string, string | null>>({});
  useEffect(() => {
    if (mode !== "changes") return;
    let cancelled = false;
    for (const m of members) {
      const id = m.kind === "session" ? m.s.id : m.h.id;
      const read = m.kind === "session" ? api.sessionDiff(id) : api.hostedDiff(id);
      read
        .then((text) => !cancelled && setDiffs((d) => ({ ...d, [id]: text })))
        .catch((e) => !cancelled && onError(describe(e)));
    }
    return () => {
      cancelled = true;
    };
    // The member list is derived from props; its identity changes per render,
    // so the effect keys on what actually matters — who, and what state.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, statuses, mode]);

  if (members.length === 0 && scope.kind === "group") {
    return <div className="empty">This group has no members left.</div>;
  }

  return (
    <div className="group-pane">
      <header className="group-head">
        <span className="group-prompt">{prompt}</span>
        <span className="quiet">
          {members.length === 0
            ? "nothing running here yet"
            : `${members.length} agent${members.length === 1 ? "" : "s"} · each in its own worktree`}
        </span>
        {/* One more member, asked the same prompt. The group grows the way it
            started: each newcomer in a worktree of its own. */}
        <div className="new-menu group-add" ref={addHost}>
          <button className="act" disabled={busy || !root} onClick={() => setAdding((a) => !a)} aria-haspopup="menu" aria-expanded={adding}>
            <IconPlus size={12} />
            {busy ? "Adding…" : "Add"}
          </button>
          {adding && (
            <ul className="menu" role="menu">
              <li role="none">
                <button role="menuitem" onClick={() => void add(null)}>
                  <IconStart size={13} />
                  <span className="menu-title">axio session</span>
                  <span className="menu-detail">{scope.kind === "group" ? "its own worktree, the same prompt" : "its own worktree, waiting for a prompt"}</span>
                </button>
              </li>
              {available.length > 0 && <li className="menu-rule" role="separator" />}
              {available.map((a) => (
                <li role="none" key={a.harness} style={{ ["--agent-accent" as string]: `var(${a.accentVar})` }}>
                  <button role="menuitem" onClick={() => void add(a.harness)}>
                    <IconTerminal size={13} />
                    <span className="menu-title">{a.label}</span>
                    <span className="menu-detail">in a terminal, its own worktree</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <button
          className="act"
          title={layout ? "Back to the grid of cards" : "Split panes: drag a card's header onto another's edge, drag the dividers"}
          onClick={() => onLayout(layout ? null : balanced(ids, "row"))}
        >
          {layout ? <IconGrid size={13} /> : <IconSplit size={13} />}
          {layout ? "Grid" : "Split"}
        </button>
        <div className="views" role="tablist">
          <button role="tab" aria-selected={mode === "overview"} className={mode === "overview" ? "view on" : "view"} onClick={() => setMode("overview")}>
            Overview
          </button>
          <button role="tab" aria-selected={mode === "live"} className={mode === "live" ? "view on" : "view"} onClick={() => setMode("live")}>
            <IconMessage size={13} />
            Live
          </button>
          <button role="tab" aria-selected={mode === "changes"} className={mode === "changes" ? "view on" : "view"} onClick={() => setMode("changes")}>
            <IconDiff size={13} />
            Changes
          </button>
        </div>
      </header>
      {layout ? (
        <div className="group-split">
          <SplitView
            node={layout}
            path={[]}
            render={(id) => {
              const m = members.find((x) => (x.kind === "session" ? x.s.id : x.h.id) === id);
              return m ? card(m) : <div className="empty">gone</div>;
            }}
            onRatio={(path, ratio) => onLayout(withRatio(layout, path, ratio))}
            dragging={dragging}
            hover={hover}
            onHover={setHover}
            onDrop={(target, side) => {
              if (dragging) onLayout(place(layout, dragging, target, side));
              setDragging(null);
              setHover(null);
            }}
          />
        </div>
      ) : (
        <div className={mode === "overview" ? "group-grid dense" : "group-grid"} style={{ ["--cols" as string]: Math.min(members.length, 3) }}>
          {members.map(card)}
        </div>
      )}
      {/* The master box: one line to every member that is listening. */}
      {members.length > 0 && (
        <div className="master-ask">
          <IconListen size={13} />
          <input
            value={master}
            disabled={masterBusy}
            placeholder={`To ${members.length - muted.size} of ${members.length} — the listening ones`}
            aria-label="Type to every listening member"
            onChange={(e) => setMaster(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void broadcast();
              }
            }}
          />
        </div>
      )}
      {menu && <RowMenu items={menu.items} at={menu.at} onClose={() => setMenu(null)} />}
    </div>
  );

  function card(m: (typeof members)[number]) {
    {
          const id = m.kind === "session" ? m.s.id : m.h.id;
          const name = m.kind === "session" ? (m.s.title ?? "axio") : m.h.name;
          const branch = m.kind === "session" ? m.s.branch : m.h.branch;
          const accent =
            m.kind === "session"
              ? m.s.isolation === "direct"
                ? "var(--agent-pi)"
                : "var(--agent-axio)"
              : `var(${m.h.accentVar})`;
          const needs = attention.has(id);
          const status =
            m.kind === "session" ? (m.s.open ? m.s.status : "closed") : m.h.status === "running" ? (m.h.agentStatus ?? "running") : m.h.status;
          const text = diffs[id];
          const files = text ? (text.match(/^diff --git /gm)?.length ?? (text.trim() ? 1 : 0)) : 0;
          const body =
            mode === "changes" ? (
              <div className="group-card-diff">
                <Diff text={text ?? null} />
              </div>
            ) : m.kind === "terminal" ? (
              // A terminal card follows the pane's switch until it is told
              // its own view from its menu, and then keeps that.
              (views[id] ?? (mode === "live" ? "tui" : "card")) === "card" ? (
                <TerminalSummary terminal={m.h} settings={terminal} compact onError={onError} />
              ) : (
                <div className="group-card-term">
                  <HostedTerminal key={id} session={m.h} terminal={terminal} compact />
                </div>
              )
            ) : mode === "overview" ? (
              <SessionSummary
                session={m.s}
                needs={needs}
                approvals={approvals.filter((a) => a.sessionId === id)}
                onChanged={onChanged}
                onError={onError}
              />
            ) : (
              <LiveTranscript id={id} onError={onError} />
            );
          // The card's own tab, at the top of its menu: a card is a way of
          // looking at a member, and expanding it is the one thing only a
          // card can offer.
          const tab: Tab = m.kind === "session" ? { kind: "session", id } : { kind: "terminal", id };
          const size = sizes[id] ?? "small";
          const custom = typeof size === "object" ? size : null;
          const preset = typeof size === "string" ? size : null;
          const lead: MenuItem[] = [
            { kind: "action", title: "Expand into its own tab", run: () => onOpen(tab) },
            ...(m.kind === "terminal"
              ? [
                  { kind: "rule" as const },
                  ...viewItems(views[id] ?? (mode === "live" ? "tui" : "card"), ["card", "tui"], (v) => onView(id, v)),
                  { kind: "rule" as const },
                ]
              : []),
            ...SIZES.map((s) => ({
              kind: "action" as const,
              title: `${s.size === preset ? "● " : "○ "}${s.title}`,
              run: () => onSize(id, s.size),
            })),
            ...(custom ? [{ kind: "action" as const, title: `● Custom, ${custom.w}×${custom.h}` }] : []).map((x) => ({
              ...x,
              run: () => {},
            })),
          ];
          // Drag the corner: the card takes exactly the size it is dragged
          // to, and keeps it. The presets are the same thing with names.
          const startDrag = (e: React.PointerEvent<HTMLElement>) => {
            const card = (e.currentTarget as HTMLElement).closest(".group-card") as HTMLElement | null;
            if (!card) return;
            e.preventDefault();
            e.stopPropagation();
            const box = card.getBoundingClientRect();
            const host = card.parentElement?.getBoundingClientRect();
            const maxW = host ? host.width - 32 : Infinity;
            const from = { x: e.clientX, y: e.clientY, w: box.width, h: box.height };
            const move = (ev: PointerEvent) => {
              const w = Math.round(Math.min(maxW, Math.max(MIN_W, from.w + (ev.clientX - from.x))));
              const h = Math.round(Math.max(MIN_H, from.h + (ev.clientY - from.y)));
              onSize(id, { w, h });
            };
            const up = () => {
              window.removeEventListener("pointermove", move);
              window.removeEventListener("pointerup", up);
              document.body.classList.remove("resizing");
            };
            document.body.classList.add("resizing");
            window.addEventListener("pointermove", move);
            window.addEventListener("pointerup", up);
          };
          const items = m.kind === "session" ? sessionMenu(m.s, hooks, lead) : hostedMenu(m.h, hooks, lead);
          return (
            <section
              className={`group-card size-${preset ?? "custom"}`}
              key={id}
              style={{
                ["--agent-accent" as string]: accent,
                ...(custom ? { width: custom.w, height: custom.h, flex: "none" } : {}),
              }}
              onContextMenu={(e) => openMenu(e, items)}
            >
              {/* Not a button: clicking a header must not yank the view to
                  that member. Expanding is on the menu, deliberately. In the
                  split layout the header is the handle a card is moved by. */}
              <div
                className="group-card-head"
                draggable={layout !== null}
                onDragStart={(e) => {
                  e.dataTransfer.effectAllowed = "move";
                  e.dataTransfer.setData("text/plain", id);
                  setDragging(id);
                }}
                onDragEnd={() => {
                  setDragging(null);
                  setHover(null);
                }}
              >
                <span className={`dot ${needs ? "needs" : status}`} />
                {m.kind === "terminal" && <IconTerminal size={12} />}
                <span className="name">{name}</span>
                <span className="state">{needs ? "needs you" : status === "running" || status === "working" ? "working" : status}</span>
                <span className="card-acts">
                  <button className="row-more" aria-label={`Options for ${name}`} title="Options" onClick={(e) => openMenu(e, items)}>
                    <IconMore size={12} />
                  </button>
                  <button
                    className="row-more"
                    aria-label={`Resize ${name}`}
                    title={`Size: ${preset ?? `${custom?.w}×${custom?.h}`} — click for the next preset, drag the corner for any size`}
                    onClick={() => onSize(id, NEXT[preset ?? "small"])}
                  >
                    <IconResize size={12} />
                  </button>
                  <button
                    className={`row-more${muted.has(id) ? "" : " on"}`}
                    aria-pressed={!muted.has(id)}
                    aria-label={muted.has(id) ? `${name} ignores the master box` : `${name} listens to the master box`}
                    title={muted.has(id) ? "Not listening to the box below — click to listen" : "Listening to the box below — click to mute"}
                    onClick={() => toggleMute(id)}
                  >
                    <IconListen size={12} />
                  </button>
                  <button className="row-more" aria-label={`Expand ${name}`} title="Expand into its own tab" onClick={() => onOpen(tab)}>
                    <IconExpand size={12} />
                  </button>
                  <button
                    className="row-more"
                    aria-label={m.kind === "session" ? `Close ${name}` : `Stop ${name}`}
                    title={m.kind === "session" ? "Close, keep the worktree" : "Stop the terminal"}
                    onClick={() => (m.kind === "session" ? onCloseSession(id, false) : onStopTerminal(id))}
                  >
                    <IconClose size={12} />
                  </button>
                </span>
              </div>
              <div className="group-card-facts">
                {branch && (
                  <span className="branch">
                    <IconBranch size={11} />
                    {branch}
                  </span>
                )}
                {mode === "changes" && (
                  <span className="quiet">
                    {text === undefined ? "reading…" : `${files} file${files === 1 ? "" : "s"} changed`}
                  </span>
                )}
              </div>
              {body}
              {layout === null && (
                <span
                  className="card-grip"
                  role="presentation"
                  title="Drag to resize"
                  onPointerDown={startDrag}
                />
              )}
            </section>
          );
    }
  }
}

// The split layout, drawn: a tree of rows and columns with a divider at
// each split that drags, and at each leaf a card and — while another card
// is being dragged — five drop zones saying where it would land.
function SplitView({
  node,
  path,
  render,
  onRatio,
  dragging,
  hover,
  onHover,
  onDrop,
}: {
  node: LayoutNode;
  path: ("a" | "b")[];
  render: (id: string) => React.ReactNode;
  onRatio: (path: ("a" | "b")[], ratio: number) => void;
  dragging: string | null;
  hover: { id: string; side: Side } | null;
  onHover: (h: { id: string; side: Side } | null) => void;
  onDrop: (target: string, side: Side) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  if (node.kind === "leaf") {
    const sideAt = (e: React.DragEvent): Side => {
      const box = (e.currentTarget as HTMLElement).getBoundingClientRect();
      const x = (e.clientX - box.left) / box.width;
      const y = (e.clientY - box.top) / box.height;
      if (x < 0.25) return "left";
      if (x > 0.75) return "right";
      if (y < 0.25) return "top";
      if (y > 0.75) return "bottom";
      return "center";
    };
    const over = dragging !== null && dragging !== node.id;
    return (
      <div
        className="lay-leaf"
        onDragOver={(e) => {
          if (!over) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
          const side = sideAt(e);
          if (hover?.id !== node.id || hover.side !== side) onHover({ id: node.id, side });
        }}
        onDragLeave={() => {
          if (hover?.id === node.id) onHover(null);
        }}
        onDrop={(e) => {
          if (!over) return;
          e.preventDefault();
          onDrop(node.id, sideAt(e));
        }}
      >
        {render(node.id)}
        {over && hover?.id === node.id && <div className={`drop-hint ${hover.side}`} />}
      </div>
    );
  }
  const startDivider = (e: React.PointerEvent) => {
    e.preventDefault();
    const box = host.current?.getBoundingClientRect();
    if (!box) return;
    const move = (ev: PointerEvent) => {
      const ratio = node.dir === "row" ? (ev.clientX - box.left) / box.width : (ev.clientY - box.top) / box.height;
      onRatio(path, ratio);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.classList.remove(node.dir === "row" ? "resizing-x" : "resizing-y");
    };
    document.body.classList.add(node.dir === "row" ? "resizing-x" : "resizing-y");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div className={`lay-split lay-${node.dir}`} ref={host}>
      <div className="lay-side" style={{ flexBasis: `${node.ratio * 100}%` }}>
        <SplitView node={node.a} path={[...path, "a"]} render={render} onRatio={onRatio} dragging={dragging} hover={hover} onHover={onHover} onDrop={onDrop} />
      </div>
      <div className="lay-divider" role="separator" aria-orientation={node.dir === "row" ? "vertical" : "horizontal"} onPointerDown={startDivider} />
      <div className="lay-side" style={{ flexBasis: `${(1 - node.ratio) * 100}%` }}>
        <SplitView node={node.b} path={[...path, "b"]} render={render} onRatio={onRatio} dragging={dragging} hover={hover} onHover={onHover} onDrop={onDrop} />
      </div>
    </div>
  );
}

// A session's transcript, kept current the way the session pane keeps its
// own: on the session's activity event, with a slow fallback for a signal
// that was missed.
function LiveTranscript({ id, onError }: { id: string; onError: (message: string) => void }) {
  const [view, setView] = useState<TranscriptView | null>(null);
  const reading = useRef(false);
  const again = useRef(false);
  const pull = useCallback(async () => {
    if (reading.current) {
      again.current = true;
      return;
    }
    reading.current = true;
    try {
      do {
        again.current = false;
        setView(await api.sessionTranscript(id));
      } while (again.current);
    } catch (e) {
      onError(describe(e));
    } finally {
      reading.current = false;
    }
  }, [id, onError]);

  useEffect(() => {
    void pull();
    const timer = window.setInterval(() => void pull(), 4000);
    const unlisten = listen<string>("axio://session-activity", (event) => {
      if (event.payload === id) void pull();
    });
    return () => {
      window.clearInterval(timer);
      void unlisten.then((off) => off());
    };
  }, [pull, id]);

  return (
    <div className="group-card-live">
      <Transcript view={view} />
    </div>
  );
}

// A session at a glance: what it is doing, the last thing it said, a place to
// answer it, and what it is running on. The whole transcript is one mode
// over; this is the card a wall of twelve agents is scanned by.
function SessionSummary({
  session,
  needs,
  approvals,
  onChanged,
  onError,
}: {
  session: SessionView;
  needs: boolean;
  approvals: ApprovalView[];
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const [view, setView] = useState<TranscriptView | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const pull = useCallback(async () => {
    try {
      setView(await api.sessionTranscript(session.id));
    } catch (e) {
      onError(describe(e));
    }
  }, [session.id, onError]);
  useEffect(() => {
    void pull();
    const timer = window.setInterval(() => void pull(), 4000);
    const unlisten = listen<string>("axio://session-activity", (event) => {
      if (event.payload === session.id) void pull();
    });
    return () => {
      window.clearInterval(timer);
      void unlisten.then((off) => off());
    };
  }, [pull, session.id]);

  const entries = view?.entries ?? [];
  const lastAgent = [...entries].reverse().find((e) => e.kind === "agent");
  const said = lastAgent && "text" in lastAgent ? lastAgent.text : "";
  const steps = entries.filter((e) => e.kind === "tool").length;
  const running = session.status === "running";
  const state = needs ? "Waiting for you" : running ? "Working" : session.open ? "Idle" : "Closed";

  const send = async () => {
    const text = draft.trim();
    if (!text) return;
    setBusy(true);
    try {
      await api.sendPrompt(session.id, text);
      setDraft("");
      onChanged();
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(false);
    }
  };
  const stop = () => void api.cancelSession(session.id).then(onChanged).catch((e) => onError(describe(e)));

  return (
    <div className="summary">
      <div className="summary-state">
        <b>{state}</b>
        <span className="quiet">
          {steps} step{steps === 1 ? "" : "s"}
          {view && view.costUsd > 0 ? ` · $${view.costUsd.toFixed(3)}` : ""}
        </span>
      </div>
      {/* A question waiting is the card's whole point at that moment: it
          replaces the last message, and is answered here. */}
      {approvals.length > 0 ? (
        <div className="summary-ask-approval">
          <Approvals approvals={approvals} onResolved={onChanged} onError={onError} />
        </div>
      ) : (
        <div className="summary-said">{said.trim() === "" ? (running ? "…" : "Nothing said yet.") : said}</div>
      )}
      <div className="summary-ask">
        <span className="prompt-mark">›</span>
        <input
          value={draft}
          placeholder={running ? "Add a follow-up · queued" : "Add a follow-up"}
          disabled={busy || !session.open}
          aria-label="Follow up"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void send();
            }
          }}
        />
        {running && (
          <button className="row-more" title="Stop the running turn" aria-label="Stop" onClick={stop}>
            <IconStop size={11} />
          </button>
        )}
      </div>
      <div className="summary-foot">
        <span>{view?.model ?? "—"}</span>
        {session.branch && (
          <span className="branch">
            <IconBranch size={10} />
            {session.branch.replace(/^axio\//, "")}
          </span>
        )}
      </div>
    </div>
  );
}

// A hosted agent in a card. Its words are its terminal's — there is no
// transcript to summarise — so the card *is* the terminal: the agent's own
// interface, live, with its menus and selections, and under it a follow-up
// box of ours that types into it and knows its slash commands. The two are
// the same prompt seen twice; the box is for a wall of cards, where typing
// into a small terminal is fiddly.
/** A key as the terminal would receive it, or `null` for one that is the
 *  browser's — Cmd-anything, function keys, a modifier alone. */
function keyBytes(e: React.KeyboardEvent, lettersToo: boolean): string | null {
  if (e.metaKey) return null;
  const named: Record<string, string> = {
    ArrowUp: "\x1b[A",
    ArrowDown: "\x1b[B",
    ArrowRight: "\x1b[C",
    ArrowLeft: "\x1b[D",
    Enter: "\r",
    Escape: "\x1b",
    Tab: "\t",
    Backspace: "\x7f",
    Delete: "\x1b[3~",
    Home: "\x1b[H",
    End: "\x1b[F",
    PageUp: "\x1b[5~",
    PageDown: "\x1b[6~",
  };
  const hit = named[e.key];
  if (hit !== undefined) return hit;
  if (e.ctrlKey && e.key.length === 1 && /[a-z]/i.test(e.key)) {
    return String.fromCharCode(e.key.toUpperCase().charCodeAt(0) - 64);
  }
  if (lettersToo && !e.ctrlKey && !e.altKey && e.key.length === 1) return e.key;
  return null;
}

/** Whether a key is one a terminal menu is driven with rather than typed text. */
function isSteering(e: React.KeyboardEvent): boolean {
  return e.key.length > 1 || (e.ctrlKey && !e.metaKey);
}

export function TerminalSummary({
  terminal,
  settings,
  compact = false,
  resumeKey,
  onError,
}: {
  terminal: HostedView;
  settings: TerminalSettings | null;
  compact?: boolean;
  /** Changes on a resume, so the emulator remounts for the new process. */
  resumeKey?: number;
  onError: (message: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const live = terminal.status === "running";
  const pickRef = useRef<HTMLLIElement>(null);
  // Relay: every key goes to the agent, letters included, so a picker the
  // agent drew after a slash command can be filtered and chosen from here.
  // Entered when a slash command is sent; left on Enter or Esc, which the
  // agent gets too, or by clicking the pill, which it does not.
  const [relay, setRelay] = useState(false);
  const relayKey = (bytes: string) => void api.hostedWrite(terminal.id, bytes, false).catch((e) => onError(describe(e)));
  // What the agent's own slash menu would offer, read once per terminal.
  // The box types into the agent's prompt, so anything on this list works
  // exactly as it would there; the list is so a person is not typing blind.
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  const [pick, setPick] = useState(0);
  useEffect(() => {
    void api.hostedCommands(terminal.id).then(setCommands).catch(() => setCommands([]));
  }, [terminal.id]);
  const slashing = draft.startsWith("/") && !draft.includes(" ");
  const matches = slashing ? commands.filter((c) => c.name.startsWith(draft)) : [];
  const chosen = matches[Math.min(pick, Math.max(0, matches.length - 1))];
  useEffect(() => {
    pickRef.current?.scrollIntoView({ block: "nearest" });
  }, [pick, chosen]);
  const send = async () => {
    const text = draft.trim();
    if (!text) return;
    try {
      // Paced: the text, a pause, then Enter — so a slash command opens the
      // agent's own menu and the Enter chooses it rather than pasting both.
      await api.hostedSubmit(terminal.id, text);
      setDraft("");
      setPick(0);
      if (text.startsWith("/")) setRelay(true);
    } catch (e) {
      onError(describe(e));
    }
  };
  const complete = (name: string) => {
    setDraft(`${name} `);
    setPick(0);
  };
  return (
    <div className="summary">
      <div className="summary-state">
        <b>
          {live
            ? terminal.agentStatus === "blocked"
              ? "Needs you"
              : terminal.agentStatus === "done"
                ? "Done"
                : terminal.agentStatus === "idle"
                  ? "Waiting for a prompt"
                  : "Working"
            : terminal.stopped
              ? "Stopped"
              : terminal.status}
        </b>
        <span className="quiet">
          {terminal.agentStatus ? "its own word, by its hooks" : "its own interface · type into it, or into the box below"}
        </span>
      </div>
      <div className="summary-term">
        {terminal.transport === "app" ? (
          <HostedChat terminal={terminal} onError={onError} />
        ) : (
          <HostedTerminal
            key={`${terminal.id}:${resumeKey ?? (live ? "live" : "dead")}`}
            session={terminal}
            terminal={settings}
            compact={compact}
          />
        )}
      </div>
      <div className="summary-ask">
        {matches.length > 0 && (
          <ul className="slash-menu" role="listbox" aria-label="Slash commands">
            {matches.map((c) => (
              <li
                key={c.name}
                ref={c === chosen ? pickRef : undefined}
                role="option"
                aria-selected={c === chosen}
                className={c === chosen ? "on" : undefined}
                onMouseDown={(e) => {
                  e.preventDefault();
                  complete(c.name);
                }}
              >
                <code>{c.name}</code>
                {c.detail && <span className="quiet">{c.detail}</span>}
              </li>
            ))}
          </ul>
        )}
        {relay ? (
          <button className="relay-pill" onClick={() => setRelay(false)} title="Keys typed here go to the agent. Click to stop; Esc stops and is sent too.">
            keys go to the agent · Esc to stop
          </button>
        ) : (
          <span className="prompt-mark">›</span>
        )}
        <input
          value={relay ? "" : draft}
          placeholder={relay ? "arrows, Enter, letters — straight to the agent" : "Type to the agent · / for its commands"}
          disabled={!live}
          aria-label="Type to the agent"
          className={relay ? "relaying" : undefined}
          onChange={(e) => {
            if (relay) return;
            setDraft(e.target.value);
            setPick(0);
          }}
          onKeyDown={(e) => {
            if (relay) {
              const bytes = keyBytes(e, true);
              if (bytes === null) return;
              e.preventDefault();
              relayKey(bytes);
              if (e.key === "Enter" || e.key === "Escape") setRelay(false);
              return;
            }
            // An empty box is the agent's: arrows, Enter, Esc, Tab and
            // Ctrl keys drive whatever it is showing, letters still type.
            if (draft === "" && isSteering(e)) {
              const bytes = keyBytes(e, false);
              if (bytes !== null) {
                e.preventDefault();
                relayKey(bytes);
                return;
              }
            }
            if (matches.length > 0) {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setPick((p) => (p + 1) % matches.length);
                return;
              }
              if (e.key === "ArrowUp") {
                e.preventDefault();
                setPick((p) => (p - 1 + matches.length) % matches.length);
                return;
              }
              if (e.key === "Tab" || (e.key === "Enter" && chosen && chosen.name !== draft)) {
                e.preventDefault();
                if (chosen) complete(chosen.name);
                return;
              }
              if (e.key === "Escape") {
                e.preventDefault();
                setDraft("");
                return;
              }
            }
            if (e.key === "Enter") {
              e.preventDefault();
              void send();
            }
          }}
        />
      </div>
      <div className="summary-foot">
        <span>{terminal.label}</span>
        {terminal.branch && (
          <span className="branch">
            <IconBranch size={10} />
            {terminal.branch.replace(/^axio\//, "")}
          </span>
        )}
      </div>
    </div>
  );
}
