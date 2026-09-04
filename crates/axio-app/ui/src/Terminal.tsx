import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, type HostedView, type TerminalSettings, listen } from "./bridge";

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

/** The font the terminal renders in, from the settings or the stylesheet's
 *  mono stack. One function, so a measurement cannot use a different face from
 *  the thing being measured. */
function face(settings: TerminalSettings | null): { font: string; size: number; lineHeight: number; scrollback: number } {
  const font =
    settings && settings.font.trim() !== ""
      ? `"${settings.font.trim().replace(/^["']|["']$/g, "")}", monospace`
      : token("--mono-font") || "monospace";
  return {
    font,
    size: settings?.fontSize ?? 13,
    // 1.0 by default: block-drawing glyphs in a Nerd Font stop being
    // contiguous at anything else, and provider TUIs are full of them.
    lineHeight: settings?.lineHeight ?? 1.0,
    scrollback: settings?.scrollback ?? 10000,
  };
}

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
export function paneSize(el: Element, settings: TerminalSettings | null): { rows: number; cols: number } | null {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const { font, size, lineHeight } = face(settings);
  ctx.font = `${size}px ${font}`;
  const cell = ctx.measureText("W").width;
  if (!(cell > 0)) return null;

  const box = el.getBoundingClientRect();
  // The padding `.terminal-host .xterm` carries, which is not available to
  // cells. Read from the stylesheet so the two cannot drift.
  const style = getComputedStyle(el);
  const padX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
  const padY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
  const cols = Math.max(20, Math.floor((box.width - padX) / cell));
  const rows = Math.max(6, Math.floor((box.height - padY) / (size * lineHeight)));
  return { rows, cols };
}

export function HostedTerminal({
  session,
  terminal,
  compact = false,
}: {
  session: HostedView;
  terminal: TerminalSettings | null;
  /** A card-sized view of the same terminal: smaller type, same bytes. */
  compact?: boolean;
}) {
  const host = useRef<HTMLDivElement>(null);

  // Read once, when the terminal opens. A change in settings reaches the
  // terminals opened after it; re-creating a live emulator would lose its
  // scrollback for the sake of a font.
  const settings = useRef(terminal);

  useEffect(() => {
    if (!host.current) return;
    const chosen = face(settings.current);
    const { font, lineHeight, scrollback } = chosen;
    const size = compact ? Math.max(9, Math.round(chosen.size * 0.8)) : chosen.size;

    const term = new Terminal({
      allowProposedApi: false,
      convertEol: false,
      cursorBlink: true,
      cursorStyle: "bar",
      fontFamily: font,
      fontSize: size,
      lineHeight,
      scrollback,
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
          let out = await api.hostedRead(session.id, cursor);
          // A cursor past the end means a new process is behind this row —
          // resumed while this emulator stayed mounted. Its output starts
          // at zero; so does this screen.
          if (out.cursor < cursor) {
            term.reset();
            cursor = 0;
            out = await api.hostedRead(session.id, 0);
          }
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
  }, [session.id, compact]);

  // Padding goes on xterm's own measured element rather than this host: the fit
  // addon measures the host, so padding here makes it overstate the row count
  // and clip a provider's interface at the bottom.
  return <div className={compact ? "terminal-host compact" : "terminal-host"} ref={host} />;
}
