import { useState } from "react";
import { api, describe, type ApprovalView } from "./bridge";

// The pooled queue, oldest first, from every session at once.
//
// A refusal carries a note. The note becomes the tool result the model reads,
// so "no — use the existing helper" steers the next step rather than ending
// it. Without the field the window could only say no, and the roadmap's whole
// claim about review being the centre of gravity was a button that discarded
// the one thing that made it true.
export function Approvals({
  approvals,
  onResolved,
  onError,
}: {
  approvals: ApprovalView[];
  onResolved: () => void;
  onError: (message: string) => void;
}) {
  if (approvals.length === 0) return null;
  return (
    <div className="approvals">
      {approvals.map((approval) => (
        <Approval key={approval.id} approval={approval} onResolved={onResolved} onError={onError} />
      ))}
    </div>
  );
}

function Approval({
  approval,
  onResolved,
  onError,
}: {
  approval: ApprovalView;
  onResolved: () => void;
  onError: (message: string) => void;
}) {
  const [feedback, setFeedback] = useState("");
  const [busy, setBusy] = useState(false);

  const answer = async (decision: Parameters<typeof api.resolveApproval>[1]) => {
    setBusy(true);
    try {
      await api.resolveApproval(approval.id, decision);
      onResolved();
    } catch (e) {
      onError(describe(e));
    } finally {
      setBusy(false);
    }
  };

  const deny = () => {
    const note = feedback.trim();
    void answer({ decision: "deny", feedback: note === "" ? null : note });
  };

  return (
    <article className="approval">
      <header>
        <strong>{approval.subject}</strong>
        <span className="who">{approval.shortSessionId}</span>
      </header>
      <p>{approval.reason}</p>
      {approval.preview?.kind === "diff" && <pre>{approval.preview.unified}</pre>}
      {/* The raw string, never a word-split of it. A split reads as a simpler
          command than the one that runs: a heredoc disappears and a redirect
          looks like an operand. */}
      {approval.preview?.kind === "command" && <pre>{approval.preview.raw}</pre>}
      {approval.preview?.kind === "text" && <pre>{approval.preview.text}</pre>}
      <div className="actions">
        <button
          className="act primary"
          disabled={busy}
          onClick={() => void answer({ decision: "allow" })}
        >
          Allow once
        </button>
        <button
          className="act"
          disabled={busy}
          onClick={() => void answer({ decision: "allowSession" })}
        >
          Allow this session
        </button>
        <input
          className="feedback"
          value={feedback}
          disabled={busy}
          aria-label="What to do instead"
          placeholder="What to do instead (sent with the refusal)"
          onChange={(e) => setFeedback(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") deny();
          }}
        />
        <button className="act danger" disabled={busy} onClick={deny}>
          Deny
        </button>
      </div>
    </article>
  );
}
