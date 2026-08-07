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

export function HostedTerminal({ session }: { session: HostedView }) {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!host.current) return;

    const term = new Terminal({
      allowProposedApi: false,
      convertEol: false,
      cursorBlink: true,
      cursorStyle: "bar",
      fontFamily: '"JetBrainsMono NFM", "JetBrains Mono", "Cascadia Mono", Consolas, monospace',
      fontSize: 13,
      // 1.0 on purpose: block-drawing glyphs in a Nerd Font stop being
      // contiguous at anything else, and provider TUIs are full of them.
      lineHeight: 1.0,
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

    const pull = async () => {
      if (stopped) return;
      try {
        const out = await api.hostedRead(session.id, cursor);
        if (out.text) term.write(out.text);
        cursor = out.cursor;
      } catch {
        // The session went away. Stop asking rather than logging once a frame.
        stopped = true;
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
