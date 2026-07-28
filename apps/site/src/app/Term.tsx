import type { ReactNode } from "react";

// Artifacts are written as plain strings so they stay legible in source and can
// be diffed against the README they came from. Two inline marks:
//   «dim»  — the terminal's secondary colour (timings, frames, comments)
//   ‹acc›  — the accent (tool markers, the one thing the eye should land on)
//
// Anything else is literal. Keeping the artifacts as text rather than JSX is
// what makes it possible to check them character-for-character against the
// program's real output, which is the whole point of showing them.

const TOKEN = /(«[^»]*»|‹[^›]*›)/g;

function render(line: string, key: number): ReactNode {
  return (
    <span key={key}>
      {line.split(TOKEN).map((part, i) => {
        if (part.startsWith("«")) {
          return (
            <span key={i} className="dim">
              {part.slice(1, -1)}
            </span>
          );
        }
        if (part.startsWith("‹")) {
          return (
            <span key={i} className="acc">
              {part.slice(1, -1)}
            </span>
          );
        }
        return part;
      })}
      {"\n"}
    </span>
  );
}

export default function Term({
  children,
  label,
  wrap = false,
}: {
  children: string;
  /** Names what the artifact is, in the frame itself rather than as a caption. */
  label?: string;
  /** For the install command, whose tail must never hide behind a scrollbar. */
  wrap?: boolean;
}) {
  const lines = children.replace(/^\n/, "").replace(/\n$/, "").split("\n");
  return (
    <figure className={`term${wrap ? " term--wrap" : ""}`}>
      {label ? <figcaption className="term__label">{label}</figcaption> : null}
      <pre>{lines.map(render)}</pre>
    </figure>
  );
}
