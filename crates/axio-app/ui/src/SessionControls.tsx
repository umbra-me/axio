import { useState } from "react";
import { api, describe, type SessionView } from "./bridge";
import { IconDiff, IconMessage } from "./icons";

// What can be done to the session in front, at the right end of the tab strip.
//
// Every control here drives a command the command line already has — close is
// `close_session`, discard is the same with the worktree deleted. That is the
// rule rather than a coincidence: a surface that could do something the CLI
// cannot has stopped being a view of the same product.

export type View = "transcript" | "changes";

export function SessionControls({
  session,
  view,
  split,
  onView,
  onClosed,
  onError,
}: {
  session: SessionView;
  view: View;
  /** Transcript and changes are side by side, so there is nothing to switch. */
  split: boolean;
  onView: (view: View) => void;
  onClosed: () => void;
  onError: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  const close = async (discard: boolean) => {
    setBusy(true);
    try {
      await api.closeSession(session.id, discard);
      onClosed();
    } catch (e) {
      // A discard the supervisor refused — the branch holds commits that live
      // nowhere else — arrives here, and is the one message worth reading.
      onError(describe(e));
      setConfirmDiscard(false);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      {!split && (
        <div className="views" role="tablist">
          <button
            role="tab"
            aria-selected={view === "transcript"}
            className={view === "transcript" ? "view on" : "view"}
            onClick={() => onView("transcript")}
            title="Transcript"
          >
            <IconMessage size={13} />
            Transcript
          </button>
          <button
            role="tab"
            aria-selected={view === "changes"}
            className={view === "changes" ? "view on" : "view"}
            onClick={() => onView("changes")}
            title="Changes"
          >
            <IconDiff size={13} />
            Changes
          </button>
        </div>
      )}
      {session.open && !confirmDiscard && (
        <>
          <button
            className="act"
            disabled={busy}
            title="End the session and keep its worktree and branch for review"
            onClick={() => void close(false)}
          >
            Close
          </button>
          <button
            className="act danger"
            disabled={busy}
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
          <button className="act" disabled={busy} onClick={() => setConfirmDiscard(false)}>
            Keep
          </button>
          <button className="act danger" disabled={busy} onClick={() => void close(true)}>
            Delete
          </button>
        </>
      )}
      {!session.open && <span className="closed-tag">closed</span>}
    </>
  );
}
