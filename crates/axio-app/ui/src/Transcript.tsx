import { useEffect, useRef } from "react";
import type { TranscriptEntry, TranscriptView } from "./bridge";
import { Markdown } from "./Markdown";

// What a session has said and done, as rows.
//
// The model's prose is rendered from its markdown by a hand-written renderer
// that produces elements, never HTML, so nothing it writes can become markup.
// Your own text stays as typed. Tool output folds away — a row wants "it ran,
// it took 12ms, it succeeded" and the forty lines are there when wanted.
export function Transcript({ view }: { view: TranscriptView | null }) {
  const end = useRef<HTMLDivElement>(null);
  const host = useRef<HTMLDivElement>(null);

  // Follow the transcript while the reader is at the bottom of it, and only
  // then. Scrolling up to re-read something is a decision; yanking the view
  // back down because a token arrived overrides it.
  const count = view?.entries.length ?? 0;
  const last = view?.entries[count - 1];
  const tail = last && "text" in last ? last.text.length : 0;
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    if (nearBottom) end.current?.scrollIntoView({ block: "end" });
  }, [count, tail]);

  if (view === null) return <div className="empty">Reading…</div>;
  if (view.entries.length === 0) {
    return (
      <div className="empty">
        {view.fromRecord ? "This session has no transcript on disk." : "Nothing yet."}
      </div>
    );
  }

  return (
    <div className="transcript" ref={host}>
      {view.fromRecord && (
        <p className="transcript-note">
          Read from the session file. This session is not running in this window.
        </p>
      )}
      {view.entries.map((entry) => (
        <Row key={entry.id} entry={entry} />
      ))}
      <div ref={end} />
    </div>
  );
}

function Row({ entry }: { entry: TranscriptEntry }) {
  switch (entry.kind) {
    case "user":
      return (
        <div className="row user">
          <span className="who">you</span>
          <div className="text">{entry.text}</div>
        </div>
      );
    case "agent":
      return (
        <div className={entry.streaming ? "row agent streaming" : "row agent"}>
          <span className="who">axio</span>
          {/* The caret is drawn by the stylesheet at the end of the last block,
              so it sits where the next word will land rather than on a line
              of its own under the paragraph. */}
          <div className="text">
            <Markdown text={entry.text} />
          </div>
        </div>
      );
    case "reasoning":
      if (entry.text.trim() === "") return null;
      return (
        <details className="row reasoning">
          <summary>thinking</summary>
          <div className="text">{entry.text}</div>
        </details>
      );
    case "tool":
      return <Tool entry={entry} />;
    case "interrupted":
      return (
        <div className="row marker warn">
          interrupted after {entry.afterSteps} step{entry.afterSteps === 1 ? "" : "s"}
        </div>
      );
    case "elision":
      return (
        <div className="row marker">
          {entry.droppedItems} earlier item{entry.droppedItems === 1 ? "" : "s"} left out of the model's
          context
        </div>
      );
    case "turn":
      return (
        <div className={`row marker turn ${entry.outcome}`}>
          <span>{turnLabel(entry.outcome)}</span>
          {entry.detail && <span className="detail">{entry.detail}</span>}
          {entry.costUsd > 0 && <span className="cost">${entry.costUsd.toFixed(4)}</span>}
        </div>
      );
    case "notice":
      return <div className={`row marker ${entry.level}`}>{entry.message}</div>;
  }
}

function turnLabel(outcome: string): string {
  switch (outcome) {
    case "completed":
      return "turn complete";
    case "refused":
      return "the model refused";
    case "interrupted":
      return "turn interrupted";
    case "step_limit":
      return "step limit reached";
    case "budget_exceeded":
      return "budget exceeded";
    case "failed":
      return "turn failed";
    default:
      return outcome;
  }
}

function Tool({ entry }: { entry: Extract<TranscriptEntry, { kind: "tool" }> }) {
  const preview = entry.preview;
  const live = entry.status === "running" || entry.status === "pending" || entry.status === "awaitingApproval";
  const bad = entry.status === "failed" || entry.status === "denied";
  return (
    <div className={`row tool ${entry.status}`}>
      <div className="tool-head">
        <span className="dot" />
        <span className="name">{entry.name}</span>
        <span className="subject">{entry.subject}</span>
        <span className="status">
          {statusLabel(entry.status)}
          {entry.ms > 0 && ` · ${entry.ms}ms`}
        </span>
      </div>
      {preview?.kind === "command" && <pre className="preview">{preview.raw}</pre>}
      {preview?.kind === "diff" && (
        <details className="preview-diff">
          <summary>
            {preview.path}
            <span className="counts">
              {preview.added > 0 && <span className="add">+{preview.added}</span>}
              {preview.removed > 0 && <span className="del">−{preview.removed}</span>}
            </span>
          </summary>
          <pre className="preview">{preview.unified}</pre>
        </details>
      )}
      {preview?.kind === "text" && <pre className="preview">{preview.text}</pre>}
      {bad && entry.output && <p className="tool-message">{entry.output}</p>}
      {!bad && !live && entry.output.trim() !== "" && (
        <details className="tool-output">
          <summary>
            output{entry.truncated ? " (truncated)" : ""}
          </summary>
          <pre>{entry.output}</pre>
        </details>
      )}
    </div>
  );
}

function statusLabel(status: string): string {
  switch (status) {
    case "awaitingApproval":
      return "waiting for approval";
    case "ok":
      return "ok";
    default:
      return status;
  }
}
