import { useCallback, useEffect, useState } from "react";
import { api, describe, type LandAction, type LandingView, type SessionView } from "./bridge";
import { copy } from "./RowMenu";
import { IconBranch } from "./icons";

// Where the work stands, and the three ways to land it.
//
// Sits above the diff. The supervisor hands over a branch and stops; this bar
// is the window's answer to "and then what" — merge into the branch the
// repository is on, push, or open a pull request — each spelled out, each one
// command, each with its result said back in a line rather than a dialog.
export function Landing({
  session,
  refreshKey,
  onError,
  onNotice,
}: {
  session: SessionView;
  /** Re-read when this changes — a turn ended, the tab came into view. */
  refreshKey: string | number;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [view, setView] = useState<LandingView | null>(null);
  const [busy, setBusy] = useState<LandAction | null>(null);

  const pull = useCallback(async () => {
    try {
      setView(await api.landing(session.id));
    } catch (e) {
      onError(describe(e));
    }
  }, [session.id, onError]);

  useEffect(() => {
    void pull();
  }, [pull, refreshKey]);

  const land = async (action: LandAction) => {
    setBusy(action);
    try {
      const out = await api.land(session.id, action);
      onNotice(out.url ? `${out.message} — ${out.url}` : out.message);
      if (out.url) copy(out.url);
      await pull();
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(null);
    }
  };

  if (!view) return null;
  const files = view.changed.length;
  const nothing = files === 0 && view.ahead === 0;

  return (
    <div className="landing">
      <div className="landing-facts">
        {view.branch ? (
          <span className="branch" title="Copy the branch name" onClick={() => copy(view.branch ?? "")}>
            <IconBranch size={12} />
            {view.branch}
          </span>
        ) : (
          <span className="quiet">in the checkout itself</span>
        )}
        <span className="arrow">→</span>
        <span className="base">{view.base}</span>
        <span className="quiet">
          {view.ahead} commit{view.ahead === 1 ? "" : "s"} ahead · {files} file{files === 1 ? "" : "s"} uncommitted
        </span>
      </div>
      {view.branch && (
        <div className="landing-acts">
          <button className="act" disabled={busy !== null || nothing} onClick={() => void land("merge")}>
            {busy === "merge" ? "Merging…" : `Merge into ${view.base}`}
          </button>
          <button
            className="act"
            disabled={busy !== null || nothing || !view.remote}
            title={view.remote ? `Push to ${view.remote}` : "This repository has no origin"}
            onClick={() => void land("push")}
          >
            {busy === "push" ? "Pushing…" : "Push"}
          </button>
          <button
            className="act primary"
            disabled={busy !== null || nothing || !view.remote || !view.canPr}
            title={view.canPr ? "Push, then open a pull request with gh" : "gh is not on this machine"}
            onClick={() => void land("pullRequest")}
          >
            {busy === "pullRequest" ? "Opening…" : "Pull request"}
          </button>
          <button className="act" onClick={() => void api.reveal(session.workspace, false).catch((e) => onError(describe(e)))}>
            Reveal
          </button>
          <button className="act" onClick={() => void api.reveal(session.workspace, true).catch((e) => onError(describe(e)))}>
            Editor
          </button>
        </div>
      )}
    </div>
  );
}
