import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, describe, type ApprovalView, type SessionView, type TranscriptView } from "./bridge";
import { Approvals } from "./Approvals";
import { Diff } from "./Diff";
import { Transcript } from "./Transcript";
import { IconBranch, IconDiff, IconMessage, IconSend, IconStop } from "./icons";

// One session: what it said, what it changed, and what can be done to it.
//
// Everything here drives a command the command line already has — a follow-up
// prompt is `send_prompt`, stop is `cancel_session`, close is `close_session`.
// That is not a coincidence to preserve but the rule: a surface that could do
// something the CLI cannot has stopped being a view of the same product.

// A fallback for a signal that was missed; the event is the mechanism.
const FALLBACK_MS = 4000;

type Tab = "transcript" | "changes";

export function SessionPane({
  session,
  approvals,
  split,
  onChanged,
  onClosed,
  onError,
}: {
  session: SessionView;
  /** This session's questions only; the others show on their own tabs. */
  approvals: ApprovalView[];
  /** Transcript and changes side by side rather than as tabs. */
  split: boolean;
  onChanged: () => void;
  onClosed: () => void;
  onError: (message: string) => void;
}) {
  const [tab, setTab] = useState<Tab>("transcript");
  const [transcript, setTranscript] = useState<TranscriptView | null>(null);
  const [diff, setDiff] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState<"send" | "stop" | "close" | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

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

  // The diff is re-read when the tab is opened and whenever a turn ends, not
  // per token: `git diff` on every delta would be the wrong kind of live.
  const turns = transcript?.entries.filter((e) => e.kind === "turn").length ?? 0;
  useEffect(() => {
    if (tab !== "changes" && !split) return;
    let cancelled = false;
    setDiff(null);
    void api
      .sessionDiff(session.id)
      .then((text) => !cancelled && setDiff(text))
      .catch((e) => !cancelled && setDiff(`could not read that worktree\n\n${describe(e)}`));
    return () => {
      cancelled = true;
    };
  }, [session.id, tab, split, turns, session.status]);

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

  const close = async (discard: boolean) => {
    setBusy("close");
    try {
      await api.closeSession(session.id, discard);
      onClosed();
    } catch (e) {
      // A discard the supervisor refused — the branch holds commits that live
      // nowhere else — arrives here, and is the one message worth reading.
      onError(describe(e));
      setConfirmDiscard(false);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="session">
      <div className="session-bar">
        <div className="crumbs">
          <span className="repo">{session.projectName}</span>
          {session.branch && (
            <span className="branch">
              <IconBranch size={12} />
              {session.branch}
            </span>
          )}
          {transcript?.model && <span className="model">{transcript.model}</span>}
          {transcript && transcript.costUsd > 0 && (
            <span className="cost" title="What this session has cost, as seen here">
              ${transcript.costUsd.toFixed(3)}
            </span>
          )}
        </div>
        {!split && (
          <div className="tabs" role="tablist">
            <button
              role="tab"
              aria-selected={tab === "transcript"}
              className={tab === "transcript" ? "tab on" : "tab"}
              onClick={() => setTab("transcript")}
            >
              <IconMessage size={13} />
              Transcript
            </button>
            <button
              role="tab"
              aria-selected={tab === "changes"}
              className={tab === "changes" ? "tab on" : "tab"}
              onClick={() => setTab("changes")}
            >
              <IconDiff size={13} />
              Changes
            </button>
          </div>
        )}
        <div className="session-actions">
          {session.open && !confirmDiscard && (
            <>
              <button
                className="act"
                disabled={busy !== null}
                title="End the session and keep its worktree and branch for review"
                onClick={() => void close(false)}
              >
                Close
              </button>
              <button
                className="act danger"
                disabled={busy !== null}
                title="End the session and delete its worktree and branch"
                onClick={() => setConfirmDiscard(true)}
              >
                Discard
              </button>
            </>
          )}
          {session.open && confirmDiscard && (
            <>
              <span className="confirm">Delete the worktree and branch?</span>
              <button className="act" disabled={busy !== null} onClick={() => setConfirmDiscard(false)}>
                Keep
              </button>
              <button className="act danger" disabled={busy !== null} onClick={() => void close(true)}>
                Delete
              </button>
            </>
          )}
          {!session.open && <span className="closed-tag">closed</span>}
        </div>
      </div>

      <div className={split ? "session-body split" : "session-body"}>
        {split ? (
          <>
            <Transcript view={transcript} />
            <Diff text={diff} />
          </>
        ) : tab === "transcript" ? (
          <Transcript view={transcript} />
        ) : (
          <Diff text={diff} />
        )}
      </div>

      {/* A question from this session sits where the answer will be typed. */}
      <Approvals approvals={approvals} onResolved={onChanged} onError={onError} />

      {live ? (
        <div className="composer">
          {running && (
            <div className="working">
              <span className="dot running" />
              working
              <button className="act" disabled={busy !== null} onClick={() => void stop()}>
                <IconStop size={12} />
                Stop
              </button>
            </div>
          )}
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
          <button
            className="act primary"
            disabled={busy !== null || prompt.trim() === ""}
            onClick={() => void send()}
            aria-label="Send"
          >
            <IconSend size={14} />
          </button>
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
