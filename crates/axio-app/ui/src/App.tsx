import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, accentFor, type ApprovalView, type HostedView, type SessionView, type Snapshot } from "./bridge";
import { HostedTerminal } from "./Terminal";
import {
  IconBranch,
  IconClose,
  IconMaximize,
  IconMinimize,
  IconRepo,
  IconStart,
  IconStop,
  IconTerminal,
} from "./icons";

// The whole surface is a function of one snapshot from Rust.
//
// There is no store here, no reducer, and no session list this file owns. That
// is the architecture rather than a simplification: the process that owns the
// sessions owns the record of them, so there is never a moment where the
// interface believes something the supervisor does not.

// A fallback, not the mechanism. The supervisor emits an event whenever any
// session does anything and the window refreshes on that; this only covers what
// no event describes — a worktree changed from outside, a session started by
// the command line while the window was already open.
const FALLBACK_MS = 5000;

export default function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [diff, setDiff] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [hosted, setHosted] = useState<HostedView[]>([]);
  const [available, setAvailable] = useState<HostedView[]>([]);
  // A hosted terminal, when one is open, owns the pane. It is somebody's live
  // interface; showing a diff over the top of it would be taking the screen
  // away mid-keystroke.
  const [terminal, setTerminal] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [snap, live] = await Promise.all([api.snapshot(), api.hostedList()]);
      setSnapshot(snap);
      setHosted(live);
      setError(null);
    } catch (e) {
      // A failed read is not a reason to blank the interface — the last good
      // snapshot is still the truest thing on screen.
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // Polled, deliberately, until Rust pushes. A cursor-and-push model is the
    // right end state; a poll that is honest about being a poll beats a push
    // that silently drops what happened while nothing was listening.
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

  useEffect(() => {
    if (!selected) {
      setDiff(null);
      return;
    }
    let cancelled = false;
    void api
      .sessionDiff(selected)
      .then((text) => !cancelled && setDiff(text))
      .catch((e) => !cancelled && setDiff(`could not read that worktree\n\n${String(e)}`));
    return () => {
      cancelled = true;
    };
  }, [selected]);

  const session = useMemo(
    () => snapshot?.sessions.find((s) => s.id === selected) ?? null,
    [snapshot, selected],
  );

  // Resolved rather than asserted. A terminal can leave the list without this
  // component being the one that ended it — the process exits, or it is stopped
  // from another row — and a non-null assertion on that lookup renders
  // `undefined` into a component that dereferences it.
  const liveTerminal = useMemo(
    () => (terminal ? (hosted.find((h) => h.id === terminal) ?? null) : null),
    [hosted, terminal],
  );

  const running = snapshot?.sessions.filter((s) => s.status === "running").length ?? 0;

  return (
    <div className="window">
      <TitleBar session={session} />

      <div className="body">
        <Rail
          snapshot={snapshot}
          selected={selected}
          onSelect={(id) => {
            setSelected(id);
            // Leaving the terminal is the point. Without this the pane showed a
            // terminal forever once one had been opened, and a session's diff
            // became unreachable for the rest of the window's life.
            setTerminal(null);
          }}
          onStarted={(id) => {
            setSelected(id);
            void refresh();
          }}
          onError={setError}
          hosted={hosted}
          available={available}
          terminal={terminal}
          onOpenTerminal={setTerminal}
        />

        <main className="main">
          <div className="crumbs">
            {session ? (
              <>
                <span className="repo">{session.projectName}</span>
                <span className="sep">/</span>
                <span className="here">{session.label ?? session.shortId}</span>
                {session.branch && (
                  <span className="branch">
                    <IconBranch size={12} />
                    {session.branch}
                  </span>
                )}
              </>
            ) : liveTerminal ? (
              <>
                <span className="repo">{liveTerminal.label}</span>
                <span className="sep">/</span>
                <span className="here">{liveTerminal.cwd}</span>
              </>
            ) : (
              <span className="sep">nothing selected</span>
            )}
          </div>

          {/* A terminal is somebody's live interface and owns the pane edge to
              edge; everything else is a document and gets the reading margin. */}
          <div className={terminal ? "pane flush" : "pane"}>
            {snapshot?.unavailable && (
              <p className="notice">Sessions are unavailable: {snapshot.unavailable}</p>
            )}
            {error && <p className="notice">{error}</p>}

            <Approvals
              approvals={snapshot?.approvals ?? []}
              onResolved={() => void refresh()}
            />

            {liveTerminal ? (
              <HostedTerminal key={liveTerminal.id} session={liveTerminal} />
            ) : session ? (
              <Diff text={diff} />
            ) : (
              <Opening />
            )}
          </div>
        </main>
      </div>

      <StatusBar
        projects={snapshot?.projects.length ?? 0}
        sessions={snapshot?.sessions.length ?? 0}
        running={running}
        approvals={snapshot?.approvals.length ?? 0}
      />
    </div>
  );
}

function TitleBar({ session }: { session: SessionView | null }) {
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
            {session.shortId}
          </>
        ) : (
          "sessions"
        )}
      </div>
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
    </header>
  );
}

// What the window opens on, every time, until something is selected.
//
// This was three lines of small grey text in the middle of a black rectangle —
// the product's first impression was an empty pane. An empty state on a surface
// with no content yet is a first run, and its job is to say what this is and
// hand over the two ways to start.
function Opening() {
  return (
    <div className="opening">
      <h1>Every agent working on your code, in one window.</h1>
      <p>
        Supervised sessions run in their own git worktree on their own branch, so
        several work at once without touching the checkout you are in.
      </p>
      <ul>
        <li>
          <IconStart size={14} />
          <code>axio session start -p "…"</code> from a shell
        </li>
        <li>
          <IconStart size={14} />
          <code>/new</code> in the terminal interface
        </li>
        <li>
          <IconTerminal size={14} />
          Or start another agent's tool from the rail
        </li>
      </ul>
    </div>
  );
}

function Rail({
  snapshot,
  selected,
  onSelect,
  onStarted,
  onError,
  hosted,
  available,
  terminal,
  onOpenTerminal,
}: {
  snapshot: Snapshot | null;
  selected: string | null;
  onSelect: (id: string) => void;
  onStarted: (id: string) => void;
  onError: (message: string) => void;
  hosted: HostedView[];
  available: HostedView[];
  terminal: string | null;
  onOpenTerminal: (id: string | null) => void;
}) {
  return (
    <nav className="rail">
      <NewSession
        projects={snapshot?.projects ?? []}
        onStarted={onStarted}
        onError={onError}
      />

      <div className="rail-head">
        <span>Repositories</span>
        <span className="count">{snapshot?.projects.length ?? 0}</span>
      </div>

      <div className="rail-list">
        {(snapshot?.projects ?? []).map((project) => (
          <section className="project" key={project.id}>
            <h2 title={project.root}>
              <IconRepo size={13} />
              <span className="name">{project.name}</span>
              <span className="count">
                {project.openSessions}/{project.totalSessions}
              </span>
            </h2>
            <div className="sessions">
              {(snapshot?.sessions ?? [])
                .filter((s) => s.projectId === project.id)
                .map((s) => (
                  <button
                    key={s.id}
                    className={`session${s.id === selected ? " active" : ""}`}
                    style={{ ["--agent-accent" as string]: accentFor(s) }}
                    onClick={() => onSelect(s.id)}
                    title={s.workspace}
                  >
                    <span className={`dot ${s.status}`} />
                    <span className="label">{s.label ?? "(no prompt)"}</span>
                    <small>{s.shortId}</small>
                  </button>
                ))}
            </div>
          </section>
        ))}
        {(snapshot?.projects.length ?? 0) === 0 && !snapshot?.unavailable && (
          <p className="rail-empty">No sessions yet.</p>
        )}
      </div>

      <HostedRail
        hosted={hosted}
        available={available}
        terminal={terminal}
        projects={snapshot?.projects ?? []}
        onOpenTerminal={onOpenTerminal}
        onError={onError}
      />

      <div className="rail-foot">isolated worktrees · one branch each</div>
    </nav>
  );
}

// Start a session in its own worktree.
//
// The repository is chosen from the ones already known rather than typed,
// because a path typed into a text field is a path nobody validated — and the
// first thing that would happen to a wrong one is a worktree cut somewhere
// surprising. A picker for a new repository is native and comes with the file
// dialog, which is the one plugin this app grants itself.
function NewSession({
  projects,
  onStarted,
  onError,
}: {
  projects: Snapshot["projects"];
  onStarted: (id: string) => void;
  onError: (message: string) => void;
}) {
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const repo = useRef<HTMLSelectElement>(null);

  if (projects.length === 0) return null;

  const start = async () => {
    const path = repo.current?.value;
    if (!path || prompt.trim() === "") return;
    setBusy(true);
    try {
      const session = await api.startSession({
        path,
        prompt: prompt.trim(),
        isolation: null,
      });
      setPrompt("");
      onStarted(session.id);
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="rail-head">
        <span>New session</span>
      </div>
      <div className="new-session">
        <select ref={repo} defaultValue={projects[0]?.root} aria-label="Repository">
          {projects.map((p) => (
            <option key={p.id} value={p.root}>
              {p.name}
            </option>
          ))}
        </select>
        <input
          value={prompt}
          aria-label="What the session should do"
          placeholder="What should it do?"
          disabled={busy}
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void start();
          }}
        />
      </div>
    </>
  );
}

// Other agents' own tools, each in a terminal axio owns.
//
// Listed apart from supervised sessions on purpose. A hosted Claude Code is not
// an axio session with a different colour - it has its own approvals, its own
// history and its own idea of what a session is - and putting the two in one
// list would imply axio can do things to it that it cannot.
function HostedRail({
  hosted,
  available,
  terminal,
  projects,
  onOpenTerminal,
  onError,
}: {
  hosted: HostedView[];
  available: HostedView[];
  terminal: string | null;
  projects: Snapshot["projects"];
  onOpenTerminal: (id: string | null) => void;
  onError: (message: string) => void;
}) {
  const start = async (harness: string) => {
    const cwd = projects[0]?.root;
    if (!cwd) {
      onError("open a project first - a terminal needs somewhere to run");
      return;
    }
    try {
      const session = await api.hostedStart(harness, cwd);
      onOpenTerminal(session.id);
    } catch (e) {
      onError(String(e));
    }
  };

  // Not named `close`: that resolves to `window.close` without an error, and
  // the collision only surfaces as an argument-count mismatch at the call.
  const stopHosted = async (id: string) => {
    try {
      await api.hostedKill(id);
    } catch (e) {
      onError(String(e));
    }
    if (id === terminal) {
      onOpenTerminal(null);
    }
  };

  return (
    <div className="hosted">
      <div className="rail-head">
        <span>Agents</span>
        <span className="count">{hosted.length}</span>
      </div>
      <div className="hosted-launch">
        {available.map((a) => (
          <button
            key={a.harness}
            style={{ ["--agent-accent" as string]: `var(${a.accentVar})` }}
            onClick={() => void start(a.harness)}
            title={`Run ${a.label} in a terminal`}
          >
            <IconTerminal size={12} />
            {a.label}
          </button>
        ))}
      </div>
      {hosted.map((h) => (
        <div className="hosted-row" key={h.id}>
          <button
            className={`session${h.id === terminal ? " active" : ""}`}
            style={{ ["--agent-accent" as string]: `var(${h.accentVar})` }}
            onClick={() => onOpenTerminal(h.id)}
            title={h.cwd}
          >
            <span className={`dot ${h.status === "running" ? "running" : "closed"}`} />
            <span className="label">{h.label}</span>
            <small>{h.status === "running" ? "live" : h.status}</small>
          </button>
          <button
            className="hosted-close"
            onClick={() => void stopHosted(h.id)}
            title={`Stop ${h.label} and everything it started`}
            aria-label={`Stop ${h.label}`}
          >
            <IconStop size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}

function Approvals({
  approvals,
  onResolved,
}: {
  approvals: ApprovalView[];
  onResolved: () => void;
}) {
  if (approvals.length === 0) return null;

  const answer = async (id: string, decision: Parameters<typeof api.resolveApproval>[1]) => {
    await api.resolveApproval(id, decision);
    onResolved();
  };

  return (
    <div className="approvals">
      {approvals.map((approval) => (
        <article className="approval" key={approval.id}>
          <header>
            <strong>{approval.subject}</strong>
            <span className="who">{approval.shortSessionId}</span>
          </header>
          <p>{approval.reason}</p>
          {approval.preview?.kind === "diff" && <pre>{approval.preview.unified}</pre>}
          {/* The raw string, never a word-split of it. A split reads as a
              simpler command than the one that runs: a heredoc disappears and a
              redirect looks like an operand. */}
          {approval.preview?.kind === "command" && <pre>{approval.preview.raw}</pre>}
          {approval.preview?.kind === "text" && <pre>{approval.preview.text}</pre>}
          <div className="actions">
            <button className="act primary" onClick={() => void answer(approval.id, { decision: "allow" })}>
              Allow once
            </button>
            <button className="act" onClick={() => void answer(approval.id, { decision: "allowSession" })}>
              Allow this session
            </button>
            <button
              className="act danger"
              onClick={() => void answer(approval.id, { decision: "deny", feedback: null })}
            >
              Deny
            </button>
          </div>
        </article>
      ))}
    </div>
  );
}

function Diff({ text }: { text: string | null }) {
  if (text === null) return <div className="empty">Reading…</div>;
  if (text.trim() === "") {
    // A real outcome and a different one from a failure to read, which an empty
    // pane would be indistinguishable from.
    return <div className="empty">That session changed nothing.</div>;
  }
  return (
    <pre className="diff">
      {text.split("\n").map((line, i) => (
        <div
          key={i}
          className={
            line.startsWith("+") && !line.startsWith("+++")
              ? "add"
              : line.startsWith("-") && !line.startsWith("---")
                ? "del"
                : line.startsWith("@@")
                  ? "hunk"
                  : undefined
          }
        >
          {line || " "}
        </div>
      ))}
    </pre>
  );
}

function StatusBar({
  projects,
  sessions,
  running,
  approvals,
}: {
  projects: number;
  sessions: number;
  running: number;
  approvals: number;
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
          <b>{running}</b> running
        </span>
        <span>
          <b>{sessions}</b> total
        </span>
      </div>
      <div>{approvals > 0 && <span className="warn">{approvals} waiting</span>}</div>
    </footer>
  );
}
