"use client";

import { useEffect, useRef, useState } from "react";

// The page opens with a turn, played the way the real one runs: invoked, the
// prompt already sent, the status line thinking, tool calls landing as they
// finish, the answer streaming, the status settling.
//
// It leads rather than illustrating. The product's defining behaviour is that
// it streams a transcript inline instead of repainting a screen, so the most
// characteristic thing about it is a turn happening — and a page that explains
// that in prose first, then shows a static block, has the order backwards.
//
// There is no wordmark. The name appears as the command, which is where the
// program itself puts it.

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
    <>
      {/* The rail starts here, at full width, then narrows to the right column
          and runs down the page. Two grid children rather than one wrapper, so
          the dark block and the caption beneath it sit on the sheet directly. */}
      <div className="hero">
        <div className="hero__inner">
          <figure className="term" aria-label="An example axio turn">
            <figcaption className="term__label">a turn, played once</figcaption>
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

          <div className="hero__caption">
            <h1 className="hero__standfirst">
              An AI coding agent that stays inside your terminal instead of
              taking it over.
            </h1>
            <p className="hero__meta">
              <span>Rust 1.88+ · Linux · macOS · Windows</span>
              <span>Apache-2.0</span>
              <span>
                <b>Pre-release — nothing is tagged</b>
              </span>
            </p>
          </div>
        </div>
      </div>
    </>
  );
}
