import { useCallback, useEffect, useMemo, useState } from "react";
import { api, accentFor, type ApprovalView, type SessionView, type Snapshot } from "./bridge";

// The whole surface is a function of one snapshot from Rust.
//
// There is no store here, no reducer, and no session list this file owns. That
// is the architecture rather than a simplification: the process that owns the
// sessions owns the record of them, so there is never a moment where the
// interface believes something the supervisor does not.

const POLL_MS = 1200;

export default function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [diff, setDiff] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await api.snapshot());
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
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);

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
        <Rail snapshot={snapshot} selected={selected} onSelect={setSelected} />

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

            {session ? (
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
}: {
  snapshot: Snapshot | null;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  return (
    <nav className="rail">
      <div className="rail-head">
        <span>Projects</span>
        <small style={{ color: "var(--muted)", fontSize: 10 }}>
          {snapshot?.projects.length ?? 0}
        </small>
      </div>

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

      <div className="rail-foot">isolated worktrees · one branch each</div>
    </nav>
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
