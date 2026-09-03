import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, describe, type HostedView, type SessionView, type Snapshot } from "./bridge";
import { Approvals } from "./Approvals";
import { CloseGuard, type Closing } from "./CloseGuard";
import { Palette, type Command } from "./Palette";
import { Rail } from "./Rail";
import { SessionPane } from "./SessionPane";
import { Tabs, sameTab, type Tab } from "./Tabs";
import { HostedTerminal, paneSize } from "./Terminal";
import { isMac } from "./platform";
import { actionFor, label } from "./shortcuts";
import { IconClose, IconMaximize, IconMinimize, IconSend, IconStart, IconTerminal } from "./icons";

// The whole surface is a function of one snapshot from Rust.
//
// There is no store here, no reducer, and no session list this file owns. That
// is the architecture rather than a simplification: the process that owns the
// sessions owns the record of them, so there is never a moment where the
// interface believes something the supervisor does not.
//
// What this file does hold is *which things are being looked at* — the open
// tabs, the active one, which sessions finished while nobody was watching, a
// dismissed error — because those are facts about the window, not the work.

// A fallback, not the mechanism. The supervisor emits an event whenever any
// session does anything and the window refreshes on that; this only covers what
// no event describes — a worktree changed from outside, a session started by
// the command line while the window was already open.
const FALLBACK_MS = 5000;

export default function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [hosted, setHosted] = useState<HostedView[]>([]);
  const [available, setAvailable] = useState<HostedView[]>([]);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [active, setActive] = useState<Tab | null>(null);
  const [split, setSplit] = useState(false);
  const [palette, setPalette] = useState(false);
  // Which repository new work starts in. One choice shared by the composer and
  // the terminal launcher, and set by "add a repository" so the thing just
  // picked is the thing about to be used.
  const [repo, setRepo] = useState<string>("");
  // Sessions that finished a turn while another tab was in front. Cleared the
  // moment they are looked at. This is the one piece of "state" the webview
  // holds about sessions, and it is about attention rather than about work.
  const [unread, setUnread] = useState<Set<string>>(new Set());
  const lastStatus = useRef<Map<string, string>>(new Map());
  const activeRef = useRef<Tab | null>(null);
  activeRef.current = active;

  // Two kinds of wrong, kept apart. A read that failed clears itself the
  // moment a read succeeds — the last good snapshot is still on screen and the
  // next one will replace it. An *action* that failed does not: "could not
  // start that session" must stay until it has been read.
  const [readError, setReadError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [closing, setClosing] = useState<Closing | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [snap, live] = await Promise.all([api.snapshot(), api.hostedList()]);
      // A turn that ended while its tab was not in front is worth a mark.
      const fresh: string[] = [];
      for (const s of snap.sessions) {
        const before = lastStatus.current.get(s.id);
        if (before === "running" && s.status !== "running") {
          const looking = sameTab(activeRef.current, { kind: "session", id: s.id });
          if (!looking) fresh.push(s.id);
        }
        lastStatus.current.set(s.id, s.status);
      }
      if (fresh.length > 0) setUnread((u) => new Set([...u, ...fresh]));
      setSnapshot(snap);
      setHosted(live);
      setReadError(null);
    } catch (e) {
      setReadError(describe(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), FALLBACK_MS);
    const unlisten = listen("axio://session-activity", () => void refresh());
    return () => {
      window.clearInterval(timer);
      void unlisten.then((off) => off());
    };
  }, [refresh]);

  useEffect(() => {
    void api.hostedAvailable().then(setAvailable).catch(() => {});
  }, []);

  // Rust refused to close the window because work is running, and said so.
  useEffect(() => {
    const unlisten = listen("axio://close-requested", () => {
      void Promise.all([api.snapshot(), api.hostedList()])
        .then(([snap, live]) =>
          setClosing({
            sessions: snap.sessions.filter((s) => s.status === "running").length,
            terminals: live.filter((h) => h.status === "running").length,
          }),
        )
        .catch(() => setClosing({ sessions: 0, terminals: 0 }));
    });
    return () => void unlisten.then((off) => off());
  }, []);

  // --- tabs ---------------------------------------------------------------

  const open = useCallback((tab: Tab) => {
    setTabs((t) => (t.some((x) => sameTab(x, tab)) ? t : [...t, tab]));
    setActive(tab);
    if (tab.kind === "session") {
      setUnread((u) => {
        if (!u.has(tab.id)) return u;
        const next = new Set(u);
        next.delete(tab.id);
        return next;
      });
    }
  }, []);

  const closeTab = useCallback(
    (tab: Tab) => {
      setTabs((t) => {
        const at = t.findIndex((x) => sameTab(x, tab));
        if (at < 0) return t;
        const next = t.filter((_, n) => n !== at);
        if (sameTab(activeRef.current, tab)) {
          // The neighbour to the left, as a browser does; the composer when
          // nothing is left.
          setActive(next[Math.max(0, at - 1)] ?? null);
        }
        return next;
      });
    },
    [],
  );

  // A terminal that is gone takes its tab with it.
  useEffect(() => {
    setTabs((t) => {
      const kept = t.filter((tab) => tab.kind !== "terminal" || hosted.some((h) => h.id === tab.id));
      if (kept.length === t.length) return t;
      if (activeRef.current && !kept.some((x) => sameTab(x, activeRef.current))) setActive(null);
      return kept;
    });
  }, [hosted]);

  const projects = snapshot?.projects ?? [];
  const chosen = projects.some((p) => p.root === repo) ? repo : (projects[0]?.root ?? "");
  const sessions = snapshot?.sessions ?? [];
  const approvals = snapshot?.approvals ?? [];
  const attention = useMemo(() => new Set(approvals.map((a) => a.sessionId)), [approvals]);

  const session = useMemo(
    () => (active?.kind === "session" ? (sessions.find((s) => s.id === active.id) ?? null) : null),
    [sessions, active],
  );
  const liveTerminal = useMemo(
    () => (active?.kind === "terminal" ? (hosted.find((h) => h.id === active.id) ?? null) : null),
    [hosted, active],
  );

  // --- actions ------------------------------------------------------------

  const addRepository = useCallback(async () => {
    try {
      const project = await api.addRepository();
      if (!project) return;
      setRepo(project.root);
      setActive(null);
      await refresh();
    } catch (e) {
      setNotice(describe(e));
    }
  }, [refresh]);

  const startTerminal = useCallback(
    async (harness: string) => {
      if (!chosen) {
        setNotice("add a repository first - a terminal needs somewhere to run");
        return;
      }
      try {
        // Measured from the pane the terminal is about to take over, so the
        // harness paints its opening screen at the size it will actually have.
        const pane = document.querySelector(".pane");
        const h = await api.hostedStart(harness, chosen, "", pane ? paneSize(pane) : null);
        await refresh();
        open({ kind: "terminal", id: h.id });
      } catch (e) {
        setNotice(describe(e));
      }
    },
    [chosen, open, refresh],
  );

  const stopTerminal = useCallback(
    async (id: string) => {
      try {
        await api.hostedKill(id);
      } catch (e) {
        setNotice(describe(e));
      }
      closeTab({ kind: "terminal", id });
      void refresh();
    },
    [closeTab, refresh],
  );

  // --- the keyboard -------------------------------------------------------

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const action = actionFor(e);
      if (action === null) return;
      e.preventDefault();
      if (typeof action === "object") {
        const tab = tabs[action.tab];
        if (tab) open(tab);
        return;
      }
      switch (action) {
        case "newSession":
          setActive(null);
          break;
        case "newTerminal":
          if (available[0]) void startTerminal(available[0].harness);
          break;
        case "palette":
          setPalette((p) => !p);
          break;
        case "closeTab":
          if (active) closeTab(active);
          break;
        case "split":
          setSplit((s) => !s);
          break;
        case "nextTab":
        case "prevTab": {
          if (tabs.length === 0) break;
          const at = active ? tabs.findIndex((t) => sameTab(t, active)) : -1;
          const step = action === "nextTab" ? 1 : -1;
          const next = tabs[(at + step + tabs.length) % tabs.length];
          if (next) open(next);
          break;
        }
        case "addRepository":
          void addRepository();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [tabs, active, available, open, closeTab, startTerminal, addRepository]);

  // --- the palette --------------------------------------------------------

  const commands = useMemo<Command[]>(() => {
    const out: Command[] = [
      { id: "new", group: "Start", title: "New session", detail: label("newSession"), run: () => setActive(null) },
      { id: "repo", group: "Start", title: "Add a repository", detail: label("addRepository"), run: () => void addRepository() },
      ...available.map((a) => ({
        id: `term:${a.harness}`,
        group: "Start",
        title: `Open ${a.label} in a terminal`,
        detail: a.harness === available[0]?.harness ? label("newTerminal") : undefined,
        run: () => void startTerminal(a.harness),
      })),
      { id: "split", group: "View", title: split ? "Show transcript and changes as tabs" : "Show transcript and changes side by side", detail: label("split"), run: () => setSplit((s) => !s) },
    ];
    if (active) {
      out.push({ id: "close", group: "View", title: "Close this tab", detail: label("closeTab"), run: () => closeTab(active) });
    }
    for (const s of sessions) {
      if (!s.open) continue;
      const state = attention.has(s.id) ? "needs you" : s.status === "running" ? "working" : s.status;
      out.push({
        id: `go:${s.id}`,
        group: "Sessions",
        title: s.label ?? s.shortId,
        detail: `${s.projectName} · ${state}`,
        run: () => open({ kind: "session", id: s.id }),
      });
      if (s.status === "running") {
        out.push({
          id: `stop:${s.id}`,
          group: "Sessions",
          title: `Stop: ${s.label ?? s.shortId}`,
          detail: s.projectName,
          run: () => void api.cancelSession(s.id).then(refresh).catch((e) => setNotice(describe(e))),
        });
      }
    }
    for (const h of hosted) {
      out.push({
        id: `hosted:${h.id}`,
        group: "Terminals",
        title: h.label,
        detail: `${h.cwd.split(/[\\/]/).filter(Boolean).pop() ?? ""} · ${h.status === "running" ? "live" : h.status}`,
        run: () => open({ kind: "terminal", id: h.id }),
      });
    }
    return out;
  }, [available, split, active, sessions, hosted, attention, addRepository, startTerminal, closeTab, open, refresh]);

  const running = sessions.filter((s) => s.status === "running").length;

  return (
    <div className={isMac ? "window mac" : "window"}>
      <TitleBar session={session} terminal={liveTerminal} />

      <div className="body">
        <Rail
          snapshot={snapshot}
          active={active}
          attention={attention}
          unread={unread}
          onAddRepository={() => void addRepository()}
          onNew={() => setActive(null)}
          onOpen={open}
          hosted={hosted}
          available={available}
          onStartTerminal={(harness) => void startTerminal(harness)}
          onStopTerminal={(id) => void stopTerminal(id)}
        />

        <main className="main">
          <Tabs
            tabs={tabs}
            active={active}
            sessions={sessions}
            hosted={hosted}
            attention={attention}
            unread={unread}
            onActivate={open}
            onClose={closeTab}
            onNew={() => setActive(null)}
          />

          {/* A terminal is somebody's live interface and owns the pane edge to
              edge; a session pane manages its own scrolling; the composer is a
              document and gets the reading margin. */}
          <div className={liveTerminal ? "pane flush" : session ? "pane session-pane" : "pane opening-pane"}>
            {snapshot?.unavailable && (
              <p className="notice">Sessions are unavailable: {snapshot.unavailable}</p>
            )}
            {readError && <p className="notice">{readError}</p>}
            {notice && (
              <p className="notice dismissable">
                <span>{notice}</span>
                <button onClick={() => setNotice(null)} aria-label="Dismiss">
                  <IconClose size={12} />
                </button>
              </p>
            )}

            {liveTerminal ? (
              <HostedTerminal key={liveTerminal.id} session={liveTerminal} />
            ) : session ? (
              <SessionPane
                key={session.id}
                session={session}
                approvals={approvals.filter((a) => a.sessionId === session.id)}
                split={split}
                onChanged={() => void refresh()}
                onClosed={() => {
                  closeTab({ kind: "session", id: session.id });
                  void refresh();
                }}
                onError={setNotice}
              />
            ) : (
              <>
                {/* Every question from every session, when no session is in
                    front: the pooled queue the supervisor promised. */}
                <Approvals approvals={approvals} onResolved={() => void refresh()} onError={setNotice} />
                <Opening
                  projects={projects}
                  repo={chosen}
                  onRepoChange={setRepo}
                  onAddRepository={() => void addRepository()}
                  onStarted={(id) => {
                    open({ kind: "session", id });
                    void refresh();
                  }}
                  onError={setNotice}
                />
              </>
            )}
          </div>
        </main>
      </div>

      <StatusBar
        projects={projects.length}
        sessions={sessions.filter((s) => s.open).length}
        running={running}
        approvals={approvals.length}
        onPalette={() => setPalette(true)}
      />

      {palette && <Palette commands={commands} onClose={() => setPalette(false)} />}
      {closing && <CloseGuard closing={closing} onKeep={() => setClosing(null)} />}
    </div>
  );
}

function TitleBar({ session, terminal }: { session: SessionView | null; terminal: HostedView | null }) {
  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="wordmark" data-tauri-drag-region>
        <i />
        axio
      </div>
      <div className="title" data-tauri-drag-region>
        {session ? (
          <>
            {session.projectName}
            <span>/</span>
            {session.label ?? session.shortId}
          </>
        ) : terminal ? (
          <>
            {terminal.label}
            <span>/</span>
            {terminal.cwd.split(/[\\/]/).filter(Boolean).pop()}
          </>
        ) : null}
      </div>
      {/* On macOS the traffic lights are native and sit at the left; drawing a
          second set here would be two ways to close one window. */}
      {!isMac && (
        <div className="window-actions">
          <button onClick={() => void api.windowControl("minimize")} aria-label="Minimise">
            <IconMinimize size={14} />
          </button>
          <button onClick={() => void api.windowControl("toggle-maximize")} aria-label="Maximise">
            <IconMaximize size={13} />
          </button>
          <button className="close" onClick={() => void api.windowControl("close")} aria-label="Close">
            <IconClose size={14} />
          </button>
        </div>
      )}
    </header>
  );
}

// What the window opens on, every time, until something is selected.
//
// It is the composer. Starting a session is the thing this window is for, so
// the pane asks the question and the rail lists the answers. When no
// repository is known yet, the first step is adding one, and the composer says
// so instead of hiding.
function Opening({
  projects,
  repo,
  onRepoChange,
  onAddRepository,
  onStarted,
  onError,
}: {
  projects: Snapshot["projects"];
  repo: string;
  onRepoChange: (root: string) => void;
  onAddRepository: () => void;
  onStarted: (id: string) => void;
  onError: (message: string) => void;
}) {
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const none = projects.length === 0;

  const start = async () => {
    const text = prompt.trim();
    if (!repo || text === "") return;
    setBusy(true);
    try {
      const session = await api.startSession({ path: repo, prompt: text, isolation: null });
      setPrompt("");
      onStarted(session.id);
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="opening">
      <h1>{none ? "Add a repository to begin" : "Start a session"}</h1>
      <p>
        Each session works in its own git worktree on its own branch, so several run at once
        without touching the checkout you are in. Review what it changed here, and land it your
        own way.
      </p>

      <div className={none ? "start disabled" : "start"}>
        <div className="start-where">
          <select
            value={none ? "" : repo}
            disabled={none || busy}
            onChange={(e) => onRepoChange(e.target.value)}
            aria-label="Repository for new work"
          >
            {none && <option value="">No repository yet</option>}
            {projects.map((p) => (
              <option key={p.id} value={p.root}>
                {p.name}
              </option>
            ))}
          </select>
          <button className="act" onClick={onAddRepository} disabled={busy}>
            {none ? "Add a repository" : "Add another"}
          </button>
        </div>
        <textarea
          value={prompt}
          rows={4}
          autoFocus
          disabled={none || busy}
          aria-label="What the session should do"
          placeholder={none ? "Pick a repository first." : "What should it do?"}
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void start();
            }
          }}
        />
        <div className="start-go">
          <span className="hint">Enter starts · Shift+Enter for a new line · {label("palette")} for everything else</span>
          <button
            className="act primary"
            disabled={none || busy || prompt.trim() === ""}
            onClick={() => void start()}
          >
            <IconSend size={13} />
            Start
          </button>
        </div>
      </div>

      <ul>
        <li>
          <IconStart size={14} />
          <code>axio session start -p "…"</code> does the same from a shell
        </li>
        <li>
          <IconTerminal size={14} />
          Or run another agent's tool in a terminal: {label("newTerminal")}, or pick one from the rail
        </li>
      </ul>
    </div>
  );
}

function StatusBar({
  projects,
  sessions,
  running,
  approvals,
  onPalette,
}: {
  projects: number;
  sessions: number;
  running: number;
  approvals: number;
  onPalette: () => void;
}) {
  return (
    <footer className="statusbar">
      <div>
        <span className="quiet">local</span>
        <span>
          <b>{projects}</b> repositor{projects === 1 ? "y" : "ies"}
        </span>
      </div>
      <div>
        <span className={running > 0 ? "ok" : "quiet"}>
          <b>{running}</b> working
        </span>
        <span>
          <b>{sessions}</b> open
        </span>
        {approvals > 0 && (
          <span className="warn">
            <b>{approvals}</b> need{approvals === 1 ? "s" : ""} you
          </span>
        )}
      </div>
      <div>
        <button className="statusbar-key" onClick={onPalette}>
          <kbd>{label("palette")}</kbd> commands
        </button>
      </div>
    </footer>
  );
}

