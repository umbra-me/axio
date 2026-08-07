"use client";

import { useEffect, useRef, useState } from "react";

// A turn, played the way the real one runs: invoked, the prompt already sent,
// the status line thinking, tool calls landing as they finish, the answer
// streaming, the status settling.

const BOX_WIDTH = 54;

const MODEL = "claude-opus-5";
const METER = "4s · 1.2k in / 340 out";
const PROMPT = "› explain the change";

// Every frame line is built to exactly BOX_WIDTH so the corners meet. Composed
// from measured parts rather than hand-counted dashes: an early version had a
// 63-character top against a 54-character bottom and the box did not close.
const TOP = (() => {
  const left = `╭─ ${MODEL} `;
  const right = ` ${METER} ─╮`;
  return left + "─".repeat(BOX_WIDTH - left.length - right.length) + right;
})();

const MID_LEFT = `│ ${PROMPT}`;
const MID_PAD = " ".repeat(BOX_WIDTH - MID_LEFT.length - 1);
const BOTTOM = `╰${"─".repeat(BOX_WIDTH - 2)}╯`;

const TOOL_LINES = [
  { name: "read", arg: "src/parse.rs", delta: "", ms: "3ms" },
  { name: "edit", arg: "src/parse.rs", delta: "+12 −4", ms: "18ms" },
];

const ANSWER = "Done — the lexer now owns the span table.";

type Phase = "thinking" | "streaming" | "done";

// The drifting scrollback that used to sit behind the hero lived here too. The
// hero now has a slab of the desktop surface behind its glass, and two ambient
// textures competing for the same space is one more than the page can spend.

export default function HeroTranscript() {
  const [tools, setTools] = useState(0);
  const [typed, setTyped] = useState(0);
  const [phase, setPhase] = useState<Phase>("thinking");
  const timers = useRef<number[]>([]);

  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setTools(TOOL_LINES.length);
      setTyped(ANSWER.length);
      setPhase("done");
      return;
    }

    const at = (ms: number, fn: () => void) => {
      timers.current.push(window.setTimeout(fn, ms));
    };

    at(500, () => setTools(1));
    at(840, () => setTools(2));
    at(1080, () => setPhase("streaming"));
    for (let i = 1; i <= ANSWER.length; i += 1) {
      at(1080 + i * 16, () => setTyped(i));
    }
    at(1080 + ANSWER.length * 16 + 260, () => setPhase("done"));

    const scheduled = timers.current;
    return () => scheduled.forEach(clearTimeout);
  }, []);

  // Both strings are ones the product actually shows. An earlier version had
  // the settled line read "ready", which axio does not say — inventing UI copy
  // on a page whose argument is that it only claims what it can show is a small
  // hypocrisy, and this page cannot afford it.
  const status =
    phase === "done"
      ? `  ${"".padEnd(32)}/ for commands`
      : `  ${"· thinking".padEnd(32)}ctrl-c to interrupt`;

  return (
    <figure className="terminal" aria-label="An example axio turn">
      <figcaption className="terminal__bar">
        <span className="terminal__dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
        a turn, played once
      </figcaption>
      <pre>
        <span className="dim">{"$ "}</span>
        {"axio\n\n"}
        {TOOL_LINES.map((line, i) => (
          <span key={line.name} className={i < tools ? "in" : "out"}>
            {"  "}
            <span className="acc">⏺</span>
            {` ${line.name.padEnd(8)}${line.arg}`}
            {line.delta ? <span className="dim">{`  ${line.delta}`}</span> : ""}
            <span className="dim">
              {" ".repeat(
                Math.max(
                  1,
                  38 - line.arg.length - (line.delta ? line.delta.length + 2 : 0),
                ),
              )}
              {line.ms}
            </span>
            {"\n"}
          </span>
        ))}
        {"\n"}
        <span className={phase === "thinking" ? "out" : "in"}>
          {"  "}
          {ANSWER.slice(0, typed)}
          {phase === "streaming" && <span className="caret">▌</span>}
          {"\n"}
        </span>
        {"\n"}
        <span className="dim">{TOP}</span>
        {"\n"}
        <span className="dim">│</span>
        {` ${PROMPT}`}
        {MID_PAD}
        <span className="dim">│</span>
        {"\n"}
        <span className="dim">{BOTTOM}</span>
        {"\n"}
        <span className="dim">{status}</span>
      </pre>
    </figure>
  );
}
