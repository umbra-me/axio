import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  MOCK,
  api,
  describe,
  type HostedView,
  type TerminalSettings,
  type ProviderView,
  type SessionView,
  type SettingsView,
  type Snapshot,
  listen,
} from "./bridge";
import { GroupPane, type CardSize, type Scope, type TerminalView } from "./GroupPane";
import { TerminalPane } from "./TerminalPane";
import { EFFORTS, PERMISSIONS, formatPlan, fromColumns, memberTransport, parsePlan, type LayoutNode, type PlanMember } from "./layout";
import { Approvals } from "./Approvals";
import { CloseGuard, type Closing } from "./CloseGuard";
import { Palette, type Command } from "./Palette";
import { Rail } from "./Rail";
import { SessionControls, type View } from "./SessionControls";
import { SessionPane, type SessionMeta } from "./SessionPane";
import { Settings, applyAppearance } from "./Settings";
import { Tabs, sameTab, type Tab } from "./Tabs";
import { paneSize } from "./Terminal";
import { isMac } from "./platform";
import { actionFor, label } from "./shortcuts";
import {
  IconBranch,
  IconClose,
  IconMaximize,
  IconMinimize,
  IconRail,
  IconSend,
  IconSettings,
  IconStop,
  IconTerminal,
} from "./icons";

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
  // Which half of a session is in front when they are not side by side. One
  // choice for the window rather than one per tab: a reviewer flipping to
  // "changes" is in a reviewing mood, and wants the next tab to open there too.
  const [view, setView] = useState<View>("transcript");
  const [railOpen, setRailOpen] = useState(true);
  const [palette, setPalette] = useState(false);
  // Rust's settings, as last read. Appearance is applied the moment they
  // arrive; the dialog edits a copy and this is replaced by what was saved.
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Which providers this machine can use, for the opening pane's first-run
  // cards. Read once; a sign-in is a restart of the session, not of this.
  const [providers, setProviders] = useState<ProviderView[]>([]);
  // What the status bar says about the session in front, reported by the pane
  // that reads the transcript. Cleared when the tab changes so a stale cost
  // never sits under a different session.
  const [meta, setMeta] = useState<SessionMeta | null>(null);
  // How big each member's card is in a group view. A fact about looking,
  // like the tabs: the window's to hold, and gone with it.
  const [cardSizes, setCardSizes] = useState<Record<string, CardSize>>({});
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
    void api.providers().then(setProviders).catch(() => {});
    void api
      .settings()
      .then((view) => {
        setSettings(view);
        applyAppearance(view.settings);
      })
      .catch((e) => setNotice(describe(e)));
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
  }, []);

  // A terminal that wrote while nobody was looking is marked the way a
  // session that finished is. Its signal fires per write, so the mark is set
  // once the output has been quiet for a moment — the shape of a turn ending
  // seen from outside — and only for a terminal that is not on screen.
  const quiet = useRef<Map<string, number>>(new Map());
  useEffect(() => {
    const unlisten = listen<string>("axio://hosted-activity", (event) => {
      const id = event.payload;
      const pending = quiet.current.get(id);
      if (pending !== undefined) window.clearTimeout(pending);
      quiet.current.set(
        id,
        window.setTimeout(() => {
          quiet.current.delete(id);
          if (!onScreen.current.has(id)) setUnread((u) => (u.has(id) ? u : new Set([...u, id])));
        }, 1500),
      );
    });
    return () => {
      void unlisten.then((off) => off());
      for (const t of quiet.current.values()) window.clearTimeout(t);
    };
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

  // Under the mock, land on the view named by `VITE_MOCK_VIEW` — a session,
  // a terminal, or the opening — so each state can be looked at without a
  // hand on the mouse. A build without the mock compiles this away.
  const staged = useRef(false);
  useEffect(() => {
    if (!MOCK || staged.current || !snapshot || hosted.length === 0) return;
    staged.current = true;
    const view = import.meta.env.VITE_MOCK_VIEW ?? "opening";
    const first = snapshot.sessions.filter((s) => s.open).slice(0, 2);
    const terminal = hosted[0];
    if (view === "opening" || view === "firstrun") return;
    for (const s of first) open({ kind: "session", id: s.id });
    if (terminal) open({ kind: "terminal", id: terminal.id });
    if ((view === "session" || view === "bare") && first[0]) open({ kind: "session", id: first[0].id });
    if (view === "bare") setRailOpen(false);
    if (view === "settings") setSettingsOpen(true);
    if (view === "group" || view === "split") {
      const g = snapshot.sessions.find((s) => s.group !== null)?.group;
      if (g) {
        open({ kind: "group", id: g });
        if (view === "split") {
          const ids = [...snapshot.sessions.filter((s) => s.group === g).map((s) => s.id), ...hosted.filter((h) => h.group === g).map((h) => h.id)];
          setLayouts({ [`group:${g}`]: fromColumns([ids.slice(0, 1), ids.slice(1)]) });
        }
      }
    }
    if (view === "changes" && first[1]) {
      open({ kind: "session", id: first[1].id });
      setView("changes");
    }
    if (view === "needs" && first[0]) {
      const asking = snapshot.sessions.find((s) => snapshot.approvals.some((a) => a.sessionId === s.id));
      if (asking) open({ kind: "session", id: asking.id });
    }
  }, [snapshot, hosted, open]);

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
  // Who needs a person: a session with a question waiting, or a terminal
  // whose agent said it is blocked on a permission.
  const attention = useMemo(
    () => new Set([...approvals.map((a) => a.sessionId), ...hosted.filter((h) => h.agentStatus === "blocked").map((h) => h.id)]),
    [approvals, hosted],
  );

  // What the front tab shows: the member itself, or every member of the
  // group or repository it lays out. Being shown is being read, so the marks
  // on those clear as they are shown.
  const onScreen = useRef<Set<string>>(new Set());
  useEffect(() => {
    const shown = new Set<string>();
    if (active?.kind === "session" || active?.kind === "terminal") shown.add(active.id);
    if (active?.kind === "group") {
      for (const s of sessions) if (s.group === active.id) shown.add(s.id);
      for (const h of hosted) if (h.group === active.id) shown.add(h.id);
    }
    if (active?.kind === "project") {
      const root = projects.find((p) => p.id === active.id)?.root;
      for (const s of sessions) if (s.projectId === active.id && s.open) shown.add(s.id);
      for (const h of hosted) if (root !== undefined && h.repo === root) shown.add(h.id);
    }
    onScreen.current = shown;
    setUnread((u) => {
      if (![...u].some((id) => shown.has(id))) return u;
      return new Set([...u].filter((id) => !shown.has(id)));
    });
  }, [active, sessions, hosted, projects]);

  const session = useMemo(
    () => (active?.kind === "session" ? (sessions.find((s) => s.id === active.id) ?? null) : null),
    [sessions, active],
  );
  const liveTerminal = useMemo(
    () => (active?.kind === "terminal" ? (hosted.find((h) => h.id === active.id) ?? null) : null),
    [hosted, active],
  );
  const scope = useMemo<Scope | null>(() => {
    if (active?.kind === "group") return { kind: "group", id: active.id };
    if (active?.kind === "project") {
      const p = projects.find((x) => x.id === active.id);
      return p ? { kind: "project", id: p.id, name: p.name } : null;
    }
    return null;
  }, [active, projects]);
  useEffect(() => setMeta(null), [session?.id]);

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
    async (harness: string, isolation: "worktree" | "direct" = "worktree", root?: string) => {
      const where = root ?? chosen;
      if (!where) {
        setNotice("add a repository first - a terminal needs somewhere to run");
        return;
      }
      try {
        // Measured from the pane the terminal is about to take over, so the
        // harness paints its opening screen at the size it will actually have.
        const pane = document.querySelector(".pane");
        const size = pane ? paneSize(pane, settings?.settings.terminal ?? null) : null;
        const h = await api.hostedStart(harness, where, isolation, "", size);
        await refresh();
        open({ kind: "terminal", id: h.id });
      } catch (e) {
        setNotice(describe(e));
      }
    },
    [chosen, open, refresh, settings],
  );

  const closeSession = useCallback(
    async (id: string, discard: boolean) => {
      try {
        await api.closeSession(id, discard);
        closeTab({ kind: "session", id });
      } catch (e) {
        setNotice(describe(e));
      }
      void refresh();
    },
    [closeTab, refresh],
  );

  // Stop keeps the row and the tab: an ended terminal is still a worktree
  // and a branch, and the tab is where "resume" lives. Remove is the one
  // that forgets it.
  const stopTerminal = useCallback(
    async (id: string) => {
      try {
        await api.hostedStop(id);
      } catch (e) {
        setNotice(describe(e));
      }
      void refresh();
    },
    [refresh],
  );

  const removeTerminal = useCallback(
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

  // Where each member of a group or repository pane sits, by scope key. A
  // pane without one lays its cards out as a grid.
  const [layouts, setLayouts] = useState<Record<string, LayoutNode | null>>({});


  // Each terminal's own view — card, terminal under a header, or plain —
  // once it has chosen one; before that, the settings' default.
  const [termViews, setTermViews] = useState<Record<string, TerminalView>>({});
  const viewOf = (id: string): TerminalView => {
    const chosen = termViews[id];
    if (chosen) return chosen;
    const d = settings?.settings.terminal.view;
    return d === "tui" || d === "plain" ? d : "card";
  };

  // How the window was looking at things is the window's to keep: layouts,
  // sizes, each terminal's view, the tabs. Read once, before the first
  // snapshot is acted on; written whole, a moment after any of it changes.
  // Rust holds the file; the webview never keeps this in its own storage.
  const restored = useRef(false);
  const [restoredTabs, setRestoredTabs] = useState<{ tabs: Tab[]; active: Tab | null } | null>(null);
  useEffect(() => {
    if (MOCK) {
      restored.current = true;
      return;
    }
    void api
      .windowState()
      .then((w) => {
        setLayouts(w.layouts as Record<string, LayoutNode | null>);
        setCardSizes(w.cardSizes as Record<string, CardSize>);
        setTermViews(w.terminalViews as Record<string, TerminalView>);
        const parse = (key: string): Tab | null => {
          const at = key.indexOf(":");
          const kind = key.slice(0, at);
          const id = key.slice(at + 1);
          return kind === "session" || kind === "terminal" || kind === "group" || kind === "project" ? { kind, id } : null;
        };
        setRestoredTabs({
          tabs: w.tabs.map(parse).filter((t): t is Tab => t !== null),
          active: w.active ? parse(w.active) : null,
        });
      })
      .catch(() => {})
      .finally(() => {
        restored.current = true;
      });
  }, []);

  // The remembered tabs come back once the first snapshot says what still
  // exists; a tab for something gone is not restored.
  useEffect(() => {
    if (!restoredTabs || !snapshot) return;
    const exists = (t: Tab) =>
      t.kind === "session"
        ? snapshot.sessions.some((s) => s.id === t.id)
        : t.kind === "terminal"
          ? hosted.some((h) => h.id === t.id)
          : t.kind === "group"
            ? snapshot.sessions.some((s) => s.group === t.id) || hosted.some((h) => h.group === t.id)
            : snapshot.projects.some((p) => p.id === t.id);
    const kept = restoredTabs.tabs.filter(exists);
    setTabs((t) => (t.length === 0 ? kept : t));
    if (restoredTabs.active && kept.some((k) => sameTab(k, restoredTabs.active))) setActive(restoredTabs.active);
    setRestoredTabs(null);
  }, [restoredTabs, snapshot, hosted]);

  // Saved a moment after anything the window remembers changes.
  useEffect(() => {
    if (MOCK || !restored.current) return;
    const timer = window.setTimeout(() => {
      void api
        .saveWindowState({
          layouts,
          cardSizes,
          terminalViews: termViews,
          tabs: tabs.map((t) => `${t.kind}:${t.id}`),
          active: active ? `${active.kind}:${active.id}` : null,
        })
        .catch(() => {});
    }, 800);
    return () => window.clearTimeout(timer);
  }, [layouts, cardSizes, termViews, tabs, active]);

  // Counted per id so the emulator remounts on a resume — a new process is a
  // new stream — and not on an exit, where the last screen is worth reading.
  const [resumes, setResumes] = useState<Record<string, number>>({});
  const resumeTerminal = useCallback(
    async (id: string) => {
      try {
        const pane = document.querySelector(".pane");
        const size = pane ? paneSize(pane, settings?.settings.terminal ?? null) : null;
        await api.hostedResume(id, size);
        setResumes((r) => ({ ...r, [id]: (r[id] ?? 0) + 1 }));
        await refresh();
        open({ kind: "terminal", id });
      } catch (e) {
        setNotice(describe(e));
      }
    },
    [open, refresh, settings],
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
        case "rail":
          setRailOpen((r) => !r);
          break;
        case "settings":
          setSettingsOpen((s) => !s);
          break;
        case "attention": {
          // The oldest question first: that is the one that has waited longest.
          const asking = approvals[0]?.sessionId;
          if (asking) open({ kind: "session", id: asking });
          break;
        }
        case "railPrev":
        case "railNext": {
          // Every row the rail shows, in its order: open sessions, then terminals.
          const rows: Tab[] = [
            ...sessions.filter((s) => s.open).map((s): Tab => ({ kind: "session", id: s.id })),
            ...hosted.map((h): Tab => ({ kind: "terminal", id: h.id })),
          ];
          if (rows.length === 0) break;
          const at = active ? rows.findIndex((t) => sameTab(t, active)) : -1;
          const step = action === "railNext" ? 1 : -1;
          const next = rows[(at + step + rows.length) % rows.length];
          if (next) open(next);
          break;
        }
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
    // `/` from anywhere but a field lands in the composer; Escape leaves one.
    const onSlash = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing = target && (target.tagName === "TEXTAREA" || target.tagName === "INPUT" || target.isContentEditable);
      if (e.key === "/" && !typing && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const box = document.querySelector<HTMLTextAreaElement>(".composer textarea, .start textarea");
        if (box) {
          e.preventDefault();
          box.focus();
        }
      } else if (e.key === "Escape" && typing && target) {
        target.blur();
      }
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("keydown", onSlash);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keydown", onSlash);
    };
  }, [tabs, active, available, approvals, sessions, hosted, open, closeTab, startTerminal, addRepository]);

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
      { id: "split", group: "View", title: split ? "Show transcript or changes, one at a time" : "Show transcript and changes side by side", detail: label("split"), run: () => setSplit((s) => !s) },
      { id: "rail", group: "View", title: railOpen ? "Hide the rail" : "Show the rail", detail: label("rail"), run: () => setRailOpen((r) => !r) },
      { id: "settings", group: "View", title: "Settings", detail: label("settings"), run: () => setSettingsOpen(true) },
    ];
    if (session && !split) {
      out.push({
        id: "view",
        group: "View",
        title: view === "transcript" ? "Show changes" : "Show transcript",
        run: () => setView((v) => (v === "transcript" ? "changes" : "transcript")),
      });
    }
    if (active) {
      out.push({ id: "close", group: "View", title: "Close this tab", detail: label("closeTab"), run: () => closeTab(active) });
    }
    for (const s of sessions) {
      if (!s.open) continue;
      const state = attention.has(s.id) ? "needs you" : s.status === "running" ? "working" : s.status;
      out.push({
        id: `go:${s.id}`,
        group: "Sessions",
        title: s.title ?? s.label ?? s.shortId,
        detail: `${s.projectName} · ${state}`,
        run: () => open({ kind: "session", id: s.id }),
      });
      if (s.status === "running") {
        out.push({
          id: `stop:${s.id}`,
          group: "Sessions",
          title: `Stop: ${s.title ?? s.label ?? s.shortId}`,
          detail: s.projectName,
          run: () => void api.cancelSession(s.id).then(refresh).catch((e) => setNotice(describe(e))),
        });
      }
    }
    for (const p of projects) {
      out.push({
        id: `project:${p.id}`,
        group: "Repositories",
        title: `Side by side: ${p.name}`,
        detail: "everything running there, as cards",
        run: () => open({ kind: "project", id: p.id }),
      });
    }
    for (const h of hosted) {
      out.push({
        id: `hosted:${h.id}`,
        group: "Terminals",
        title: h.name,
        detail: `${h.branch ?? h.cwd.split(/[\\/]/).filter(Boolean).pop() ?? ""} · ${h.status === "running" ? "live" : h.status}`,
        run: () => open({ kind: "terminal", id: h.id }),
      });
      if (h.status !== "running") {
        out.push({
          id: `resume:${h.id}`,
          group: "Terminals",
          title: `Resume: ${h.name}`,
          detail: h.branch ?? h.cwd,
          run: () => void resumeTerminal(h.id),
        });
      }
    }
    return out;
  }, [available, split, railOpen, view, session, active, sessions, hosted, projects, attention, addRepository, startTerminal, resumeTerminal, closeTab, open, refresh]);

  const running = sessions.filter((s) => s.status === "running").length;

  return (
    <div className={`window${isMac ? " mac" : ""}${railOpen ? "" : " rail-hidden"}`}>
      <TitleBar
        session={session}
        terminal={liveTerminal}
        railOpen={railOpen}
        onToggleRail={() => setRailOpen((r) => !r)}
      />

      <div className="body">
        <Rail
          snapshot={snapshot}
          active={active}
          attention={attention}
          unread={unread}
          onAddRepository={() => void addRepository()}
          onNew={(root) => {
            if (root) setRepo(root);
            setActive(null);
          }}
          onOpen={open}
          hosted={hosted}
          available={available}
          onStartTerminal={(harness, isolation, root) => void startTerminal(harness, isolation, root)}
          onStopTerminal={(id) => void stopTerminal(id)}
          onResumeTerminal={(id) => void resumeTerminal(id)}
          onRemoveTerminal={(id) => void removeTerminal(id)}
          onCloseSession={(id, discard) => void closeSession(id, discard)}
          onChanged={() => void refresh()}
          onError={setNotice}
          menuOpen={MOCK && import.meta.env.VITE_MOCK_VIEW === "menu"}
        />

        <main className="main">
          <Tabs
            tabs={tabs}
            active={active}
            sessions={sessions}
            hosted={hosted}
            projects={projects}
            attention={attention}
            unread={unread}
            onActivate={open}
            onClose={closeTab}
            tail={
              session ? (
                <SessionControls
                  key={session.id}
                  session={session}
                  view={view}
                  split={split}
                  onView={setView}
                  onClosed={() => {
                    closeTab({ kind: "session", id: session.id });
                    void refresh();
                  }}
                  onError={setNotice}
                />
              ) : liveTerminal?.status === "running" ? (
                <button
                  className="act danger"
                  onClick={() => void stopTerminal(liveTerminal.id)}
                  title={`Stop ${liveTerminal.name} and everything it started`}
                >
                  <IconStop size={12} />
                  Stop
                </button>
              ) : liveTerminal ? (
                <button
                  className="act primary"
                  onClick={() => void resumeTerminal(liveTerminal.id)}
                  title={`Start ${liveTerminal.name} again in ${liveTerminal.cwd}`}
                >
                  <IconSend size={12} />
                  Resume
                </button>
              ) : null
            }
          />

          {/* A terminal is somebody's live interface and owns the pane edge to
              edge; a session pane manages its own scrolling; the composer is a
              document and gets the reading margin. */}
          <div className={liveTerminal ? "pane flush" : session || scope ? "pane session-pane" : "pane opening-pane"}>
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
              <TerminalPane
                key={liveTerminal.id}
                terminal={liveTerminal}
                view={viewOf(liveTerminal.id)}
                settings={settings?.settings.terminal ?? null}
                resumeKey={resumes[liveTerminal.id] ?? 0}
                hooks={{
                  onChanged: () => void refresh(),
                  onError: setNotice,
                  onCloseSession: (id, discard) => void closeSession(id, discard),
                  onStopTerminal: (id) => void stopTerminal(id),
                  onResumeTerminal: (id) => void resumeTerminal(id),
                  onRemoveTerminal: (id) => void removeTerminal(id),
                }}
                onView={(v) => setTermViews((t) => ({ ...t, [liveTerminal.id]: v }))}
                onResume={() => void resumeTerminal(liveTerminal.id)}
                onRemove={() => void removeTerminal(liveTerminal.id)}
              />
            ) : scope ? (
              <GroupPane
                key={`${scope.kind}:${scope.id}`}
                scope={scope}
                sessions={sessions}
                hosted={hosted}
                attention={attention}
                approvals={approvals}
                terminal={settings?.settings.terminal ?? null}
                available={available}
                projectRoot={
                  scope.kind === "project"
                    ? (projects.find((p) => p.id === scope.id)?.root ?? null)
                    : (projects.find((p) => p.id === sessions.find((s) => s.group === scope.id)?.projectId)?.root ??
                      hosted.find((h) => h.group === scope.id)?.repo ??
                      null)
                }
                onOpen={open}
                onChanged={() => void refresh()}
                onError={setNotice}
                onCloseSession={(id, discard) => void closeSession(id, discard)}
                onStopTerminal={(id) => void stopTerminal(id)}
                onResumeTerminal={(id) => void resumeTerminal(id)}
                onRemoveTerminal={(id) => void removeTerminal(id)}
                sizes={cardSizes}
                onSize={(id, size) => setCardSizes((s) => ({ ...s, [id]: size }))}
                layout={layouts[`${scope.kind}:${scope.id}`] ?? null}
                onLayout={(node) => setLayouts((l) => ({ ...l, [`${scope.kind}:${scope.id}`]: node }))}
                views={Object.fromEntries(
                  Object.entries(termViews).map(([id, v]) => [id, v === "plain" ? "tui" : v]),
                ) as Record<string, TerminalView>}
                onView={(id, v) => setTermViews((t) => ({ ...t, [id]: v }))}
              />
            ) : session ? (
              <SessionPane
                key={session.id}
                session={session}
                approvals={approvals.filter((a) => a.sessionId === session.id)}
                view={view}
                split={split}
                onChanged={() => void refresh()}
                onMeta={setMeta}
                onError={setNotice}
                onNotice={setNotice}
              />
            ) : (
              <>
                {/* Every question from every session, when no session is in
                    front: the pooled queue the supervisor promised. */}
                <Approvals approvals={approvals} onResolved={() => void refresh()} onError={setNotice} />
                <Opening
                  projects={projects}
                  repo={chosen}
                  providers={providers}
                  available={available}
                  terminal={settings?.settings.terminal ?? null}
                  onRepoChange={setRepo}
                  onAddRepository={() => void addRepository()}
                  onStarted={(tab, layout) => {
                    if (layout !== undefined) setLayouts((l) => ({ ...l, [`${tab.kind}:${tab.id}`]: layout }));
                    open(tab);
                    void refresh();
                  }}
                  onSignIn={() => {
                    // axio's own sign-in runs in axio's own surface: a terminal
                    // with the CLI, where `/login` already knows every provider.
                    void startTerminal("axio", "direct");
                  }}
                  onError={setNotice}
                />
              </>
            )}
          </div>
        </main>
      </div>

      <StatusBar
        session={session}
        meta={meta}
        running={running}
        approvals={approvals.length}
        onPalette={() => setPalette(true)}
        onSettings={() => setSettingsOpen(true)}
      />

      {palette && <Palette commands={commands} onClose={() => setPalette(false)} />}
      {settingsOpen && settings && (
        <Settings
          initial={settings}
          projects={projects}
          available={available}
          onClose={() => setSettingsOpen(false)}
          onSaved={(view) => {
            setSettings(view);
            applyAppearance(view.settings);
            setSettingsOpen(false);
          }}
          onError={setNotice}
        />
      )}
      {closing && <CloseGuard closing={closing} onKeep={() => setClosing(null)} />}
    </div>
  );
}

function TitleBar({
  session,
  terminal,
  railOpen,
  onToggleRail,
}: {
  session: SessionView | null;
  terminal: HostedView | null;
  railOpen: boolean;
  onToggleRail: () => void;
}) {
  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="wordmark" data-tauri-drag-region>
        <i />
        axio
        <button
          className={railOpen ? "rail-toggle on" : "rail-toggle"}
          onClick={onToggleRail}
          aria-pressed={railOpen}
          aria-label={railOpen ? "Hide the rail" : "Show the rail"}
          title={`${railOpen ? "Hide" : "Show"} the rail  ${label("rail")}`}
        >
          <IconRail size={14} />
        </button>
      </div>
      <div className="title" data-tauri-drag-region>
        {session ? (
          <>
            {session.projectName}
            <span>/</span>
            {session.title ?? session.label ?? session.shortId}
          </>
        ) : terminal ? (
          <>
            {terminal.name}
            <span>/</span>
            {terminal.branch ?? terminal.cwd.split(/[\\/]/).filter(Boolean).pop()}
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
// It is the composer, and little else. Starting a session is the thing this
// window is for, so the pane asks the one question and the rail lists the
// answers. Where the work happens is a control on the composer rather than a
// form above it, and the other ways in are one quiet line beneath. When no
// repository is known yet, the first step is adding one, and the composer says
// so instead of hiding.
//
// The same composer starts a group: turn the count past one, or tick the
// agents to run beside axio, and one prompt becomes several worktrees.
//
// First run is the one time this pane explains anything: which agents were
// found on this machine and which providers have a credential, because "add a
// repository" is not the first step for somebody with no key yet.
function Opening({
  projects,
  repo,
  providers,
  available,
  terminal,
  onRepoChange,
  onAddRepository,
  onStarted,
  onSignIn,
  onError,
}: {
  projects: Snapshot["projects"];
  repo: string;
  providers: ProviderView[];
  available: HostedView[];
  /** For sizing a terminal started from here before it opens. */
  terminal: TerminalSettings | null;
  onRepoChange: (root: string) => void;
  onAddRepository: () => void;
  /** With a layout when the plan drew one — columns side by side. */
  onStarted: (tab: Tab, layout?: LayoutNode | null) => void;
  onSignIn: () => void;
  onError: (message: string) => void;
}) {
  const [prompt, setPrompt] = useState("");
  // The plan: one row per member. The steppers add and remove rows; the
  // rows carry what a stepper cannot say — a model, an effort, arguments,
  // and which column the member sits in. One line of text writes them all.
  const [plan, setPlan] = useState<PlanMember[]>([{ harness: "axio", model: "", effort: "", permission: "", args: "", column: 0 }]);
  const [planOpen, setPlanOpen] = useState(false);
  const [spec, setSpec] = useState("");
  const [specError, setSpecError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const none = projects.length === 0;
  const ready = providers.some((p) => p.ready);
  const harnesses = [
    "axio",
    ...available.filter((a) => a.harness !== "axio").map((a) => a.harness),
    // Codex through its protocol, when Codex is here at all.
    ...(available.some((a) => a.harness === "codex") ? ["codex-app"] : []),
  ];
  const count = plan.filter((m) => m.harness === "axio").length;
  const picks: Record<string, number> = {};
  for (const m of plan) if (m.harness !== "axio") picks[m.harness] = (picks[m.harness] ?? 0) + 1;
  const agents = plan.filter((m) => m.harness !== "axio").map((m) => m.harness);
  const total = plan.length;
  const detailed = plan.some((m) => m.model || m.effort || m.permission || m.args || m.column > 0);
  const groupMode = total > 1 || agents.length > 0;
  const add = (harness: string) =>
    setPlan((p) =>
      p.filter((m) => m.harness === harness).length >= 8 ? p : [...p, { harness, model: "", effort: "", permission: "", args: "", column: 0 }],
    );
  const drop = (harness: string) =>
    setPlan((p) => {
      const at = p.map((m) => m.harness).lastIndexOf(harness);
      return at < 0 ? p : [...p.slice(0, at), ...p.slice(at + 1)];
    });
  const setCount = (f: (c: number) => number) => {
    const next = f(count);
    if (next > count) add("axio");
    else if (next < count) drop("axio");
  };
  const pick = (harness: string, delta: number) => (delta > 0 ? add(harness) : drop(harness));
  const editRow = (i: number, patch: Partial<PlanMember>) => setPlan((p) => p.map((m, k) => (k === i ? { ...m, ...patch } : m)));
  const applySpec = () => {
    const parsed = parsePlan(spec, harnesses);
    if ("error" in parsed) {
      setSpecError(parsed.error);
      return;
    }
    setSpecError(null);
    setPlan(parsed.members);
    setPlanOpen(true);
  };

  // A prompt is optional. Without one a session starts and waits for its
  // first turn, and a terminal opens with the tool at its own prompt — which
  // is how somebody opens Claude Code to work in by hand. One terminal on its
  // own is not a group; it is opened as itself.
  const start = async () => {
    const text = prompt.trim();
    if (!repo || total === 0) return;
    setBusy(true);
    try {
      if (count === 0 && agents.length === 1 && text === "" && !detailed && agents[0] !== "codex-app") {
        const pane = document.querySelector(".pane");
        const size = pane ? paneSize(pane, terminal) : null;
        const h = await api.hostedStart(agents[0] ?? "", repo, "worktree", "", size);
        onStarted({ kind: "terminal", id: h.id });
      } else if (groupMode || detailed) {
        const members = plan.map((m) => ({
          ...memberTransport(m.harness),
          model: m.model || null,
          effort: m.effort || null,
          permission: m.permission || null,
          args: m.args,
        }));
        const started = await api.startGroup({ path: repo, prompt: text, count: 0, agents: [], members });
        setPrompt("");
        // The plan's columns become the pane's layout, each column its
        // members stacked, in the order they were started.
        const columns: string[][] = [];
        plan.forEach((m, i) => {
          const id = started.order[i];
          if (id === undefined) return;
          (columns[m.column] ??= []).push(id);
        });
        const layout = plan.some((m) => m.column > 0) ? fromColumns(columns.filter((c) => c.length > 0)) : undefined;
        onStarted({ kind: "group", id: started.group }, layout);
      } else {
        const session = await api.startSession({ path: repo, prompt: text === "" ? null : text, isolation: null, group: null });
        setPrompt("");
        onStarted({ kind: "session", id: session.id });
      }
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="opening">
      <h1>{none ? "Add a repository to begin" : groupMode ? "What should they do?" : "What should it do?"}</h1>

      <div className={none ? "start disabled" : "start"}>
        <textarea
          value={prompt}
          rows={3}
          autoFocus
          disabled={none || busy}
          aria-label="What the session should do"
          placeholder={
            none
              ? "Pick a repository first."
              : groupMode
                ? "One prompt for all of them, or none. Each works in its own worktree; compare the results side by side."
                : "Describe the work, or leave this empty and type into it once it opens. It runs in its own worktree, on its own branch."
          }
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void start();
            }
          }}
        />
        <div className="start-go">
          {none ? (
            <button className="act" onClick={onAddRepository} disabled={busy}>
              Add a repository
            </button>
          ) : (
            <select
              value={repo}
              disabled={busy}
              onChange={(e) => onRepoChange(e.target.value)}
              aria-label="Repository for new work"
              title="Where the session works"
            >
              {projects.map((p) => (
                <option key={p.id} value={p.root}>
                  {p.name}
                </option>
              ))}
            </select>
          )}
          {/* One stepper per agent, axio included and none of them required:
              zero axio and two Claude Codes is a legitimate answer. */}
          <span className="start-many" title="How many of each agent on this prompt, each in its own worktree">
            <span className="agent-count" style={{ ["--agent-accent" as string]: "var(--agent-axio)" }}>
              <button className="stepper" disabled={none || busy || count <= 0} onClick={() => setCount((c) => Math.max(0, c - 1))} aria-label="Fewer axio sessions">
                −
              </button>
              <span className="stepper-count">{count}× axio</span>
              <button className="stepper" disabled={none || busy || count >= 8} onClick={() => setCount((c) => Math.min(8, c + 1))} aria-label="More axio sessions">
                +
              </button>
            </span>
            {available
              .filter((a) => a.harness !== "axio")
              .map((a) => {
                const n = picks[a.harness] ?? 0;
                return n === 0 ? (
                  <button
                    key={a.harness}
                    className="agent-pick"
                    style={{ ["--agent-accent" as string]: `var(${a.accentVar})` }}
                    disabled={none || busy}
                    title={`Add ${a.label}, in its own worktree`}
                    onClick={() => pick(a.harness, 1)}
                  >
                    <IconTerminal size={11} />
                    {a.label}
                  </button>
                ) : (
                  <span key={a.harness} className="agent-count on" style={{ ["--agent-accent" as string]: `var(${a.accentVar})` }}>
                    <button className="stepper" disabled={none || busy} onClick={() => pick(a.harness, -1)} aria-label={`Fewer ${a.label}`}>
                      −
                    </button>
                    <span className="stepper-count">
                      {n}× {a.label}
                    </span>
                    <button className="stepper" disabled={none || busy || n >= 8} onClick={() => pick(a.harness, 1)} aria-label={`More ${a.label}`}>
                      +
                    </button>
                  </span>
                );
              })}
          </span>
          <button
            className="act primary"
            disabled={none || busy || total === 0}
            title={prompt.trim() === "" ? "Start without a prompt: each waits to be typed at" : undefined}
            onClick={() => void start()}
          >
            <IconSend size={13} />
            {groupMode ? `Start ${total}` : "Start"}
          </button>
        </div>
        {/* The plan, spelled out: a row per member with what the steppers
            cannot say, and one line that writes the rows. Folded until it
            is wanted; the steppers are the plan for most starts. */}
        <div className="plan">
          <button className="plan-toggle" onClick={() => setPlanOpen((o) => !o)} aria-expanded={planOpen}>
            {planOpen ? "Hide the plan" : detailed ? "Plan (detailed)" : "Plan: models, effort, layout…"}
          </button>
          {planOpen && (
            <div className="plan-body">
              <div className="plan-spec">
                <input
                  value={spec}
                  placeholder="2x claude --model opus | codex -m gpt-5.4 --effort high, pi     ( | columns · , stacked )"
                  aria-label="Plan as one line"
                  onChange={(e) => setSpec(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      applySpec();
                    }
                  }}
                />
                <button className="act" onClick={applySpec} disabled={spec.trim() === ""}>
                  Apply
                </button>
                <button className="act" onClick={() => setSpec(formatPlan(plan))} title="Write the rows below as a line">
                  From rows
                </button>
              </div>
              {specError && <p className="plan-error">{specError}</p>}
              <div className="plan-rows">
                {plan.map((m, i) => (
                  <div className="plan-row" key={i}>
                    <select value={m.harness} aria-label="Agent" onChange={(e) => editRow(i, { harness: e.target.value })}>
                      {harnesses.map((h) => (
                        <option key={h} value={h}>
                          {h === "axio" ? "axio session" : h === "codex-app" ? "Codex, structured" : (available.find((a) => a.harness === h)?.label ?? h)}
                        </option>
                      ))}
                    </select>
                    <input
                      value={m.model}
                      placeholder={m.harness === "axio" ? "model (from config)" : "model"}
                      aria-label="Model"
                      disabled={m.harness === "axio"}
                      onChange={(e) => editRow(i, { model: e.target.value })}
                    />
                    <select
                      value={m.effort}
                      aria-label="Effort"
                      disabled={m.harness !== "codex" && m.harness !== "codex-app"}
                      title={m.harness === "codex" || m.harness === "codex-app" ? "Reasoning effort" : "Only Codex takes an effort on its command line"}
                      onChange={(e) => editRow(i, { effort: e.target.value })}
                    >
                      {EFFORTS.map((e) => (
                        <option key={e} value={e}>
                          {e === "" ? "effort" : e}
                        </option>
                      ))}
                    </select>
                    <select
                      value={m.permission}
                      aria-label="Permission mode"
                      disabled={m.harness === "axio" || m.harness === "pi"}
                      title={
                        m.harness === "axio" || m.harness === "pi"
                          ? "This tool has no permission flag on its command line"
                          : "What it may do without asking: ask everything, edits freely, decide itself, or nothing asks"
                      }
                      onChange={(e) => editRow(i, { permission: e.target.value })}
                    >
                      {PERMISSIONS.map((p) => (
                        <option key={p} value={p}>
                          {p === "" ? "permission" : p}
                        </option>
                      ))}
                    </select>
                    <input value={m.args} placeholder="extra arguments" aria-label="Arguments" onChange={(e) => editRow(i, { args: e.target.value })} />
                    <input
                      type="number"
                      min={1}
                      max={6}
                      value={m.column + 1}
                      aria-label="Column"
                      title="Which column it sits in, left to right; members in one column stack"
                      onChange={(e) => editRow(i, { column: Math.max(0, Math.min(5, Number(e.target.value) - 1)) })}
                    />
                    <button className="row-more" aria-label="Remove this member" onClick={() => setPlan((p) => p.filter((_, k) => k !== i))}>
                      <IconClose size={12} />
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>

      <p className="opening-more">
        <kbd>{label("newTerminal")}</kbd> another agent's tool, in a terminal · <kbd>{label("palette")}</kbd> every
        command · <code>axio session start -p "…"</code> from a shell
      </p>

      {/* First run: what this machine has, and what it lacks. Shown until a
          provider is usable, then never again. */}
      {!ready && providers.length > 0 && (
        <div className="firstrun">
          <section>
            <h2>Agents on this machine</h2>
            {available.length === 0 && <p>None found. Install one and it appears under New.</p>}
            <ul>
              {available.map((a) => (
                <li key={a.harness} style={{ ["--agent-accent" as string]: `var(${a.accentVar})` }}>
                  <span className="dot" />
                  {a.label}
                  <span className="quiet">
                    <code>{a.harness}</code>
                  </span>
                </li>
              ))}
            </ul>
          </section>
          <section>
            <h2>Providers for axio's own sessions</h2>
            <ul>
              {providers.map((p) => (
                <li key={p.name} className={p.ready ? "ready" : undefined}>
                  <span className={`dot ${p.ready ? "running" : "closed"}`} />
                  {p.name}
                  <span className="quiet">{p.ready ? "signed in" : "no credential"}</span>
                </li>
              ))}
            </ul>
            <p>
              No provider is signed in, so an axio session cannot start yet. Hosted agents work regardless.
            </p>
            <button className="act primary" onClick={onSignIn} disabled={none}>
              Sign in with axio
            </button>
            {none && <span className="quiet"> — add a repository first; the terminal needs somewhere to run</span>}
          </section>
        </div>
      )}
    </div>
  );
}

// The bar along the bottom: what is in front, what needs you, and the key.
//
// Nothing here repeats the rail. The repository count and the open count were
// both already on screen a column to the left; what the rail cannot show is
// the branch, model and cost of the session being looked at, and how many
// sessions are working or waiting across all of them.
function StatusBar({
  session,
  meta,
  running,
  approvals,
  onPalette,
  onSettings,
}: {
  session: SessionView | null;
  meta: SessionMeta | null;
  running: number;
  approvals: number;
  onPalette: () => void;
  onSettings: () => void;
}) {
  return (
    <footer className="statusbar">
      <div className="statusbar-here">
        {session ? (
          <>
            {session.branch && (
              <span className="branch" title="The session's branch">
                <IconBranch size={11} />
                {session.branch}
              </span>
            )}
            {meta?.model && <span className="mono">{meta.model}</span>}
            {meta && meta.costUsd > 0 && (
              <span className="mono" title="What this session has cost, as seen here">
                ${meta.costUsd.toFixed(3)}
              </span>
            )}
          </>
        ) : (
          <span className="quiet">local</span>
        )}
      </div>
      <div>
        {running > 0 && (
          <span className="ok">
            <b>{running}</b> working
          </span>
        )}
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
        <button className="statusbar-key" onClick={onSettings} title={`Settings  ${label("settings")}`} aria-label="Settings">
          <IconSettings size={13} />
        </button>
      </div>
    </footer>
  );
}
