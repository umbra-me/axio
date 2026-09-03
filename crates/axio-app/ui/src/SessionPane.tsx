import { useCallback, useEffect, useRef, useState } from "react";
import { api, describe, type ApprovalView, type SessionView, type TranscriptView, listen } from "./bridge";
import { Approvals } from "./Approvals";
import { Diff } from "./Diff";
import { Landing } from "./Landing";
import type { View } from "./SessionControls";
import { Transcript } from "./Transcript";
import { IconSend, IconStop } from "./icons";

// One session: what it said, what it changed, and a place to answer it.
//
// Everything here drives a command the command line already has — a follow-up
// prompt is `send_prompt`, stop is `cancel_session`. The controls that end a
// session live in the tab strip (`SessionControls`), and the facts about it —
// branch, model, cost — in the status bar; this pane is the reading and the
// typing, and nothing that repeats what the chrome around it already says.

// A fallback for a signal that was missed; the event is the mechanism.
const FALLBACK_MS = 4000;

/** What the status bar shows about the session in front. */
export type SessionMeta = { model: string | null; costUsd: number };

export function SessionPane({
  session,
  approvals,
  view,
  split,
  onChanged,
  onMeta,
  onError,
  onNotice,
}: {
  session: SessionView;
  /** This session's questions only; the others show on their own tabs. */
  approvals: ApprovalView[];
  view: View;
  /** Transcript and changes side by side rather than one at a time. */
  split: boolean;
  onChanged: () => void;
  onMeta: (meta: SessionMeta) => void;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [transcript, setTranscript] = useState<TranscriptView | null>(null);
  const [diff, setDiff] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState<"send" | "stop" | null>(null);

  // One read at a time, and one more if a signal arrived while it ran — the
  // same coalescing the terminal does, for the same reason.
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
        setTranscript(await api.sessionTranscript(session.id));
      } while (again.current);
    } catch (e) {
      onError(describe(e));
    } finally {
      reading.current = false;
    }
  }, [session.id, onError]);

  useEffect(() => {
    void pull();
    const timer = window.setInterval(() => void pull(), FALLBACK_MS);
    const unlisten = listen<string>("axio://session-activity", (event) => {
      if (event.payload === session.id) void pull();
    });
    return () => {
      window.clearInterval(timer);
      void unlisten.then((off) => off());
    };
  }, [pull, session.id]);

  // The model and the cost are the status bar's to show, and only when they
  // change — a re-render per token would repaint a row that did not move.
  const model = transcript?.model ?? null;
  const costUsd = transcript?.costUsd ?? 0;
  useEffect(() => {
    onMeta({ model, costUsd });
  }, [model, costUsd, onMeta]);

  // The diff is re-read when the changes come into view and whenever a turn
  // ends, not per token: `git diff` on every delta would be the wrong kind of
  // live.
  const turns = transcript?.entries.filter((e) => e.kind === "turn").length ?? 0;
  useEffect(() => {
    if (view !== "changes" && !split) return;
    let cancelled = false;
    setDiff(null);
    void api
      .sessionDiff(session.id)
      .then((text) => !cancelled && setDiff(text))
      .catch((e) => !cancelled && setDiff(`could not read that worktree\n\n${describe(e)}`));
    return () => {
      cancelled = true;
    };
  }, [session.id, view, split, turns, session.status]);

  const live = session.status !== "closed";
  const running = session.status === "running";

  const send = async () => {
    const text = prompt.trim();
    if (text === "" || !live) return;
    setBusy("send");
    try {
      await api.sendPrompt(session.id, text);
      setPrompt("");
      onChanged();
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(null);
    }
  };

  const stop = async () => {
    setBusy("stop");
    try {
      await api.cancelSession(session.id);
      onChanged();
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="session-view">
      <div className={split ? "session-body split" : "session-body"}>
        {split ? (
          <>
            <Transcript view={transcript} />
            <div className="changes">
              <Landing session={session} refreshKey={`${turns}:${session.status}`} onError={onError} onNotice={onNotice} />
              <Diff text={diff} />
            </div>
          </>
        ) : view === "transcript" ? (
          <Transcript view={transcript} />
        ) : (
          <div className="changes">
            <Landing session={session} refreshKey={`${turns}:${session.status}`} onError={onError} onNotice={onNotice} />
            <Diff text={diff} />
          </div>
        )}
      </div>

      {/* A question from this session sits where the answer will be typed. */}
      <Approvals approvals={approvals} onResolved={onChanged} onError={onError} />

      {live ? (
        <div className="composer">
          <textarea
            value={prompt}
            rows={2}
            disabled={busy === "send"}
            aria-label="Follow up"
            placeholder={running ? "Queued behind the running turn…" : "Follow up…"}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              // Enter sends; Shift+Enter is a new line. The shell's convention,
              // and the one every chat surface has trained everybody on.
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void send();
              }
            }}
          />
          {/* One slot, two states: stop what is running, or send what is
              typed. Two buttons side by side would leave one of them always
              disabled, which is furniture. */}
          {running ? (
            <button
              className="act stop"
              disabled={busy !== null}
              onClick={() => void stop()}
              aria-label="Stop the running turn"
              title="Stop the running turn"
            >
              <IconStop size={14} />
            </button>
          ) : (
            <button
              className="act primary"
              disabled={busy !== null || prompt.trim() === ""}
              onClick={() => void send()}
              aria-label="Send"
            >
              <IconSend size={14} />
            </button>
          )}
        </div>
      ) : (
        <div className="composer-note">
          {session.open
            ? "Not running in this window. Its worktree is still there to review, keep or discard."
            : "This session was closed."}
        </div>
      )}
    </div>
  );
}
