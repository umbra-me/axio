import type { ReactNode } from "react";
import CopyButton from "./CopyButton";

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
  copy,
}: {
  children: string;
  label?: string;
  /** For the install commands, whose tail must never hide behind a scrollbar. */
  wrap?: boolean;
  /**
   * Offer a copy button, named for what it copies so the accessible name tells
   * the three install commands apart. Omitted for the played transcript, which
   * is an artifact to read rather than a command to run.
   */
  copy?: string;
}) {
  const body = children.replace(/^\n/, "").replace(/\n$/, "");
  const lines = body.split("\n");
  return (
    <figure className={`terminal${wrap ? " terminal--wrap" : ""}`}>
      <figcaption className="terminal__bar">
        <span className="terminal__dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
        {label}
        {/* Copies what is displayed, not what is written here: the marks are
            presentation and would otherwise be pasted into a shell. */}
        {copy ? <CopyButton text={body.replace(/[«»‹›]/g, "")} what={copy} /> : null}
      </figcaption>
      <pre>{lines.map(render)}</pre>
    </figure>
  );
}
