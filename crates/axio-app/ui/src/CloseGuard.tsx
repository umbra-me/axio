import { api } from "./bridge";

export type Closing = { sessions: number; terminals: number };

// The question Rust asks when the window is closed over running work.
//
// Rust refuses the close and emits an event; this is the other half. Two
// answers only. "Keep working" is the default and the safe one; "close
// anyway" reaches for `destroy`, which skips the guard on purpose — the one
// caller allowed to, because it is the one that has just asked.
export function CloseGuard({ closing, onKeep }: { closing: Closing; onKeep: () => void }) {
  const parts: string[] = [];
  if (closing.sessions > 0) {
    parts.push(`${closing.sessions} session${closing.sessions === 1 ? "" : "s"} running a turn`);
  }
  if (closing.terminals > 0) {
    parts.push(`${closing.terminals} live terminal${closing.terminals === 1 ? "" : "s"}`);
  }
  const what = parts.length > 0 ? parts.join(" and ") : "work still running";

  return (
    <div className="scrim" role="dialog" aria-modal="true" aria-labelledby="close-title">
      <div className="modal">
        <h2 id="close-title">Close over running work?</h2>
        <p>
          There {closing.sessions + closing.terminals === 1 ? "is" : "are"} {what}. Closing now
          interrupts them. Sessions keep their worktrees and can be reviewed later; a hosted
          agent loses whatever it was in the middle of.
        </p>
        <div className="actions">
          <button className="act primary" onClick={onKeep} autoFocus>
            Keep working
          </button>
          <button className="act danger" onClick={() => void api.windowControl("destroy")}>
            Close anyway
          </button>
        </div>
      </div>
    </div>
  );
}
