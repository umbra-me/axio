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
        background: "#040507",
        foreground: "#eeeeef",
        cursor: "#7c9dff",
        cursorAccent: "#040507",
        selectionBackground: "#28375c",
        black: "#0d0e12",
        red: "#ef7178",
        green: "#62d9b3",
        yellow: "#d7bd76",
        blue: "#7c9dff",
        magenta: "#a98bfa",
        cyan: "#69c7d5",
        white: "#eeeeef",
        brightBlack: "#70727a",
        brightWhite: "#ffffff",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);
    fit.fit();

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
    void pull();
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
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(host.current);

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
