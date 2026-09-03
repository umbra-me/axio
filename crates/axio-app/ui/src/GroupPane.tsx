import { useCallback, useEffect, useRef, useState } from "react";
import { Approvals } from "./Approvals";
import {
  api,
  describe,
  listen,
  type ApprovalView,
  type HostedView,
  type SessionView,
  type TerminalSettings,
  type TranscriptView,
} from "./bridge";
import { Diff } from "./Diff";
import type { Tab } from "./Tabs";
import { hostedMenu, sessionMenu, type MenuHooks } from "./menus";
import { RowMenu, type MenuItem } from "./RowMenu";
import { HostedTerminal } from "./Terminal";
import { Transcript } from "./Transcript";
import {
  IconBranch,
  IconClose,
  IconDiff,
  IconExpand,
  IconMessage,
  IconMore,
  IconPlus,
  IconResize,
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

// Several agents, one prompt, side by side.
//
// Each member is a card: who it is, what it is doing, and — live — what it is
// saying: a session's transcript as it streams, a hosted agent's real terminal
// at a smaller size. Changes swaps every card to its diff, read when the pane
// opens and again whenever a member's status changes — a `git diff` per token
// would be the wrong kind of live. A card's header opens the member's own tab;
// the group is a way of looking, not a thing to drive.
type Mode = "overview" | "live" | "changes";

export function GroupPane({
  group,
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
  sizes,
  onSize,
}: {
  group: string;
  sessions: SessionView[];
  hosted: HostedView[];
  attention: Set<string>;
  /** Every question waiting, so a card can show its member's. */
  approvals: ApprovalView[];
  terminal: TerminalSettings | null;
  /** Agents this machine can host, for the Add menu. */
  available: HostedView[];
  /** The repository the group works on, from any member. */
  projectRoot: string | null;
  onOpen: (tab: Tab) => void;
  onChanged: () => void;
  onError: (message: string) => void;
  onCloseSession: (id: string, discard: boolean) => void;
  onStopTerminal: (id: string) => void;
  /** Card sizes by member id. Held by the window, so they survive a tab switch. */
  sizes: Record<string, CardSize>;
  onSize: (id: string, size: CardSize) => void;
}) {
  const [mode, setMode] = useState<Mode>("overview");
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);
  const hooks: MenuHooks = { onChanged, onError, onCloseSession, onStopTerminal };
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
  const members = [
    ...sessions.filter((s) => s.group === group).map((s) => ({ kind: "session" as const, s })),
    ...hosted.filter((h) => h.group === group).map((h) => ({ kind: "terminal" as const, h })),
  ];
  const first = sessions.find((s) => s.group === group);
  const prompt = first?.label ?? "group";
  const root = projectRoot;

  const add = async (harness: string | null) => {
    setAdding(false);
    if (!root) {
      onError("this group's repository is not known, so nothing can join it");
      return;
    }
    setBusy(true);
    try {
      await api.addToGroup({ group, path: root, prompt: first?.label ?? null, harness });
      onChanged();
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(false);
    }
  };
  const statuses = members.map((m) => (m.kind === "session" ? m.s.status : m.h.status)).join(",");

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
  }, [group, statuses, mode]);

  if (members.length === 0) {
    return <div className="empty">This group has no members left.</div>;
  }

  return (
    <div className="group-pane">
      <header className="group-head">
        <span className="group-prompt">{prompt}</span>
        <span className="quiet">
          {members.length} agent{members.length === 1 ? "" : "s"} · each in its own worktree
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
                  <span className="menu-detail">its own worktree, the same prompt</span>
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
      <div className={mode === "overview" ? "group-grid dense" : "group-grid"} style={{ ["--cols" as string]: Math.min(members.length, 3) }}>
        {members.map((m) => {
          const id = m.kind === "session" ? m.s.id : m.h.id;
          const name = m.kind === "session" ? (m.s.title ?? "axio") : m.h.name;
          const branch = m.kind === "session" ? m.s.branch : m.h.branch;
          const accent =
            m.kind === "session"
              ? m.s.isolation === "direct"
                ? "var(--agent-pi)"
                : "var(--agent-axio)"
              : `var(${m.h.accentVar})`;
          const needs = m.kind === "session" && attention.has(id);
          const status = m.kind === "session" ? (m.s.open ? m.s.status : "closed") : m.h.status;
          const text = diffs[id];
          const files = text ? (text.match(/^diff --git /gm)?.length ?? (text.trim() ? 1 : 0)) : 0;
          const body =
            mode === "changes" ? (
              <div className="group-card-diff">
                <Diff text={text ?? null} />
              </div>
            ) : mode === "overview" ? (
              m.kind === "session" ? (
                <SessionSummary
                  session={m.s}
                  needs={needs}
                  approvals={approvals.filter((a) => a.sessionId === id)}
                  onChanged={onChanged}
                  onError={onError}
                />
              ) : (
                <TerminalSummary terminal={m.h} onError={onError} />
              )
            ) : m.kind === "terminal" ? (
              <div className="group-card-term">
                <HostedTerminal key={id} session={m.h} terminal={terminal} compact />
              </div>
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
                  that member. Expanding is on the menu, deliberately. */}
              <div className="group-card-head">
                <span className={`dot ${needs ? "needs" : status}`} />
                {m.kind === "terminal" && <IconTerminal size={12} />}
                <span className="name">{name}</span>
                <span className="state">{needs ? "needs you" : status === "running" ? "working" : status}</span>
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
              <span
                className="card-grip"
                role="presentation"
                title="Drag to resize"
                onPointerDown={startDrag}
              />
            </section>
          );
        })}
      </div>
      {menu && <RowMenu items={menu.items} at={menu.at} onClose={() => setMenu(null)} />}
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

// A hosted agent at a glance. Its words are its terminal's — there is no
// transcript to summarise — so the card is a state, a follow-up that types
// into it, and where it is.
function TerminalSummary({ terminal, onError }: { terminal: HostedView; onError: (message: string) => void }) {
  const [draft, setDraft] = useState("");
  const live = terminal.status === "running";
  const send = async () => {
    const text = draft.trim();
    if (!text) return;
    try {
      await api.hostedWrite(terminal.id, text, true);
      setDraft("");
    } catch (e) {
      onError(describe(e));
    }
  };
  return (
    <div className="summary">
      <div className="summary-state">
        <b>{live ? "Running" : terminal.status}</b>
        <span className="quiet">read it under Live</span>
      </div>
      <div className="summary-said quiet">Its own interface. Typed below goes to its prompt.</div>
      <div className="summary-ask">
        <span className="prompt-mark">›</span>
        <input
          value={draft}
          placeholder="Type to the agent"
          disabled={!live}
          aria-label="Type to the agent"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
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
