"use client";

import { useEffect, useRef, useState } from "react";

// A turn, played the way the real one runs: the prompt is already sent, the
// status line says thinking, tool calls land one at a time as they finish, the
// answer streams, and the status clears.
//
// This is the page's one animated moment. The product's defining behaviour is
// that it streams a transcript inline rather than repainting a screen, and a
// static block describes that instead of showing it.

const BOX_WIDTH = 54;

const MODEL = "claude-opus-5";
const METER = "4s · 1.2k in / 340 out";
const PROMPT = "› explain the change";

// Every frame line is built to exactly BOX_WIDTH so the corners meet. Composed
// from measured parts rather than hand-counted dashes: the first version had a
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

export default function HeroTranscript() {
  const [tools, setTools] = useState(0);
  const [typed, setTyped] = useState(0);
  const [phase, setPhase] = useState<Phase>("thinking");
  const timers = useRef<number[]>([]);

  useEffect(() => {
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce) {
      setTools(TOOL_LINES.length);
      setTyped(ANSWER.length);
      setPhase("done");
      return;
    }

    const at = (ms: number, fn: () => void) => {
      timers.current.push(window.setTimeout(fn, ms));
    };

    at(420, () => setTools(1));
    at(760, () => setTools(2));
    at(1000, () => setPhase("streaming"));
    for (let i = 1; i <= ANSWER.length; i += 1) {
      at(1000 + i * 16, () => setTyped(i));
    }
    at(1000 + ANSWER.length * 16 + 260, () => setPhase("done"));

    const scheduled = timers.current;
    return () => scheduled.forEach(clearTimeout);
  }, []);

  // Both strings are ones the product actually shows. An earlier version had
  // the settled line read "ready", which axio does not say — inventing UI copy
  // on a page whose whole argument is that it only claims what it can show is
  // a small hypocrisy, and this page cannot afford it.
  const status =
    phase === "done"
      ? `  ${"".padEnd(32)}/ for commands`
      : `  ${"· thinking".padEnd(32)}ctrl-c to interrupt`;

  return (
    // The height is reserved for the finished turn, so nothing below moves as
    // lines arrive — a page that reflows while you read it is worse than a
    // page that does not animate at all.
    <div className="term term--wide term--hero" aria-label="An example axio turn">
      <pre>
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
    </div>
  );
}
