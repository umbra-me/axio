import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { listen } from "@tauri-apps/api/event";
import { api, type HostedView } from "./bridge";

// A hosted agent's own interface, unmodified.
//
// Nothing here parses what the agent writes. It is bytes on their way to a
// terminal emulator, and interpreting them to guess what the agent is doing
// would be a second, worse implementation of the thing already happening
// correctly on screen.
//
// Output is pulled by cursor rather than pushed. A reload loses this component
// but not the terminal - Rust owns that - so remounting asks for everything
// after the position it holds and gets exactly the gap.

// A fallback, not the mechanism. Rust signals when the ring advances, so this
// only covers a signal that was missed - which is possible by construction,
// because `notify_waiters` wakes whoever is already waiting and a listener
// registering a moment late hears nothing. The cursor makes that survivable:
// missing a signal means arriving late, never losing output.
const FALLBACK_MS = 1000;

// The stylesheet is the palette, here too.
//
// xterm renders to a canvas and cannot read a custom property itself, so the
// choice is between resolving them once here and keeping a second copy of every
// colour in this file. The second copy is what existed, and it had already
// drifted from the tokens it was copied from.
function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** The font the terminal renders in. One string, so a measurement cannot use a
 *  different face from the thing being measured. */
const TERM_FONT = '"JetBrainsMono NFM", "JetBrains Mono", "Cascadia Mono", Consolas, monospace';
const TERM_SIZE = 13;
const TERM_LINE_HEIGHT = 1.0;

/**
 * How big a terminal would be if it opened in `el` right now.
 *
 * Worth doing before the terminal exists, because a harness paints its opening
 * screen at whatever size it is told and that paint stays in scrollback. Given
 * a guess and corrected a moment later, the correction repaints the live area
 * and the mis-sized opening sits above it for the rest of the session.
 *
 * Measured with the real face at the real size rather than assumed, and against
 * a wide-ish glyph: a proportional fallback would make every column wrong, and
 * this is the same measurement xterm's fit addon makes.
 */
export function paneSize(el: Element): { rows: number; cols: number } | null {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.font = `${TERM_SIZE}px ${TERM_FONT}`;
  const cell = ctx.measureText("W").width;
  if (!(cell > 0)) return null;

  const box = el.getBoundingClientRect();
  // The padding `.terminal-host .xterm` carries, which is not available to
  // cells. Read from the stylesheet so the two cannot drift.
  const style = getComputedStyle(el);
  const padX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
  const padY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
  const cols = Math.max(20, Math.floor((box.width - padX) / cell));
  const rows = Math.max(6, Math.floor((box.height - padY) / (TERM_SIZE * TERM_LINE_HEIGHT)));
  return { rows, cols };
}

export function HostedTerminal({ session }: { session: HostedView }) {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!host.current) return;

    const term = new Terminal({
      allowProposedApi: false,
      convertEol: false,
      cursorBlink: true,
      cursorStyle: "bar",
      fontFamily: TERM_FONT,
      fontSize: TERM_SIZE,
      // 1.0 on purpose: block-drawing glyphs in a Nerd Font stop being
      // contiguous at anything else, and provider TUIs are full of them.
      lineHeight: TERM_LINE_HEIGHT,
      scrollback: 10000,
      theme: {
        background: token("--term-bg"),
        foreground: token("--term-fg"),
        cursor: token("--accent"),
        cursorAccent: token("--term-bg"),
        selectionBackground: token("--term-selection"),
        black: token("--term-black"),
        red: token("--danger"),
        green: token("--ok"),
        yellow: token("--warn"),
        blue: token("--accent"),
        magenta: token("--agent-claude"),
        cyan: token("--agent-pi"),
        white: token("--term-white"),
        brightBlack: token("--term-bright-black"),
        brightWhite: token("--term-bright-white"),
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);

    let cursor = 0;
    let stopped = false;

    // Keystrokes go straight through. `submit: false` because xterm already
    // hands us the carriage return as its own byte - adding another would send
    // the line twice.
    const typed = term.onData((data) => {
      void api.hostedWrite(session.id, data, false).catch(() => {});
    });

    // One read at a time, and one more if anything happened while it ran.
    //
    // The signal fires once per 8KB read of the pty, which for a busy agent is
    // many times a second. Answering each one with its own round trip meant
    // that while the first was still awaiting, every later signal started
    // another — all of them reading from the same stale cursor, so each came
    // back with the *same* bytes, paid full JSON serialisation for them, and
    // wrote them to the terminal again. Redundant work that grew with how busy
    // the agent was, and duplicated output on screen.
    //
    // A cursor makes the coalescing free: a signal that arrives while a read is
    // in flight is already covered by the read that follows it, so collapsing
    // any number of them into one pending flag loses nothing.
    let reading = false;
    let again = false;
    const pull = async () => {
      if (stopped) return;
      if (reading) {
        again = true;
        return;
      }
      reading = true;
      try {
        do {
          again = false;
          const out = await api.hostedRead(session.id, cursor);
          if (out.text) term.write(out.text);
          cursor = out.cursor;
        } while (again && !stopped);
      } catch {
        // The session went away. Stop asking rather than logging once a frame.
        stopped = true;
      } finally {
        reading = false;
      }
    };
    const timer = window.setInterval(() => void pull(), FALLBACK_MS);
    // The real path: read because something was written, not because a timer
    // fired. Sixteen IPC round trips a second per open terminal was the cost of
    // the alternative.
    const unlisten = listen<string>("axio://hosted-activity", (event) => {
      if (event.payload === session.id) void pull();
    });

    const resize = () => {
      fit.fit();
      void api.hostedResize(session.id, term.rows, term.cols).catch(() => {});
    };

    // Measure after the font is real, and read only after that.
    //
    // The fit addon sizes a cell by measuring one, so a fit that runs while the
    // mono face is still resolving measures the *fallback* face — and every
    // column of every box-drawing character a provider's interface is made of
    // lands at the wrong x for the rest of the session. Nothing errors; the
    // interface is simply drawn on a grid that does not match the one the
    // harness was told about.
    //
    // The first read waits on the same promise for the same reason: bytes that
    // arrive before the harness has been told its real size get wrapped at the
    // wrong width, and a wrapped line stays wrapped in scrollback forever.
    const observer = new ResizeObserver(resize);
    void document.fonts.ready.then(() => {
      if (stopped || !host.current) return;
      resize();
      observer.observe(host.current);
      void pull();
    });

    return () => {
      stopped = true;
      window.clearInterval(timer);
      void unlisten.then((off) => off());
      observer.disconnect();
      typed.dispose();
      term.dispose();
    };
  }, [session.id]);

  // Padding goes on xterm's own measured element rather than this host: the fit
  // addon measures the host, so padding here makes it overstate the row count
  // and clip a provider's interface at the bottom.
  return <div className="terminal-host" ref={host} />;
}
