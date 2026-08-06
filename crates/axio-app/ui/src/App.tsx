import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, accentFor, type ApprovalView, type HostedView, type SessionView, type Snapshot } from "./bridge";
import { HostedTerminal } from "./Terminal";

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

  const running = snapshot?.sessions.filter((s) => s.status === "running").length ?? 0;

  return (
    <div className="window">
      <TitleBar session={session} />

      <div className="body">
        <Rail
          snapshot={snapshot}
          selected={selected}
          onSelect={setSelected}
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
                <span>{session.projectName}</span>
                <span className="sep">/</span>
                <span>{session.label ?? session.shortId}</span>
                {session.branch && (
                  <>
                    <span className="sep">·</span>
                    <span className="branch">{session.branch}</span>
                  </>
                )}
              </>
            ) : (
              <span className="sep">no session selected</span>
            )}
          </div>

          <div className="pane">
            {snapshot?.unavailable && (
              <p className="notice">
                Sessions are unavailable: {snapshot.unavailable}
              </p>
            )}
            {error && <p className="notice">{error}</p>}

            <Approvals
              approvals={snapshot?.approvals ?? []}
              onResolved={() => void refresh()}
            />

            {terminal ? (
              <HostedTerminal key={terminal} session={hosted.find((h) => h.id === terminal)!} />
            ) : session ? (
              <Diff text={diff} />
            ) : (
              <div className="empty">
                <p>Nothing selected.</p>
                <p>
                  Start one with <code>axio session start -p "…"</code> or{" "}
                  <code>/new</code> in the terminal interface.
                </p>
              </div>
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
        <button onClick={() => void api.windowControl("minimize")} title="Minimise">
          ─
        </button>
        <button onClick={() => void api.windowControl("toggle-maximize")} title="Maximise">
          ▢
        </button>
        <button className="close" onClick={() => void api.windowControl("close")} title="Close">
          ✕
        </button>
      </div>
    </header>
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
      <div className="rail-head">
        <span>Projects</span>
        <small style={{ color: "var(--muted)", fontSize: 10 }}>
          {snapshot?.projects.length ?? 0}
        </small>
      </div>

      <NewSession
        projects={snapshot?.projects ?? []}
        onStarted={onStarted}
        onError={onError}
      />

      <div className="rail-list">
        {(snapshot?.projects ?? []).map((project) => (
          <section className="project" key={project.id}>
            <h2 title={project.root}>
              {project.name}
              <small>
                {project.openSessions}/{project.totalSessions}
              </small>
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
          <p style={{ padding: "12px 13px", fontSize: 11, color: "var(--muted)" }}>
            No sessions yet.
          </p>
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
    <div className="new-session">
      <select ref={repo} defaultValue={projects[0]?.root}>
        {projects.map((p) => (
          <option key={p.id} value={p.root}>
            {p.name}
          </option>
        ))}
      </select>
      <input
        value={prompt}
        placeholder="Start a session…"
        disabled={busy}
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void start();
        }}
      />
    </div>
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

  return (
    <div className="hosted">
      <div className="hosted-head">
        <span>Agents</span>
        <div className="hosted-launch">
          {available.map((a) => (
            <button
              key={a.harness}
              style={{ ["--agent-accent" as string]: `var(${a.accentVar})` }}
              onClick={() => void start(a.harness)}
              title={`Run ${a.label} in a terminal`}
            >
              {a.label}
            </button>
          ))}
        </div>
      </div>
      {hosted.map((h) => (
        <button
          key={h.id}
          className={`session${h.id === terminal ? " active" : ""}`}
          style={{ ["--agent-accent" as string]: `var(${h.accentVar})` }}
          onClick={() => onOpenTerminal(h.id)}
          title={h.cwd}
        >
          <span className={`dot ${h.status === "running" ? "running" : "closed"}`} />
          <span className="label">{h.label}</span>
          <small>{h.status === "running" ? "live" : h.status}</small>
        </button>
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
            <span style={{ color: "var(--muted)" }}>{approval.shortSessionId}</span>
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
  if (text === null) return <p style={{ color: "var(--muted)", fontSize: 11 }}>Reading…</p>;
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
        <span>local</span>
        <span>
          {projects} project{projects === 1 ? "" : "s"}
        </span>
      </div>
      <div>
        <span className={running > 0 ? "ok" : undefined}>{running} running</span>
        <span>{sessions} total</span>
      </div>
      <div>{approvals > 0 && <span className="warn">{approvals} waiting</span>}</div>
    </footer>
  );
}
