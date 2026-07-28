import type { ReactNode } from "react";

// Artifacts are written as plain strings so they stay legible in source and can
// be diffed against the README they came from. Two inline marks:
//   «dim»  — the terminal's secondary colour (comments, frames, timings)
//   ‹acc›  — the accent, for the one thing the eye should land on
//
// Keeping them as text rather than JSX is what makes it possible to check them
// character-for-character against the program's real output, which is the point
// of showing artifacts at all.

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
  label?: string;
  /** For the install commands, whose tail must never hide behind a scrollbar. */
  wrap?: boolean;
}) {
  const lines = children.replace(/^\n/, "").replace(/\n$/, "").split("\n");
  return (
    <figure className={`terminal${wrap ? " terminal--wrap" : ""}`}>
      <figcaption className="terminal__bar">
        <span className="terminal__dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
        {label}
      </figcaption>
      <pre>{lines.map(render)}</pre>
    </figure>
  );
}
