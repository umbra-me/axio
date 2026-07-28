"use client";

import { useEffect, useState } from "react";

// Three states rather than two: "system" has to stay reachable, because the
// page's default is to follow the OS and a two-state switch makes that
// unrecoverable once touched.
const STATES = ["system", "light", "dark"] as const;
type State = (typeof STATES)[number];

function read(): State {
  try {
    const stored = localStorage.getItem("axio-theme");
    if (stored && (STATES as readonly string[]).includes(stored)) {
      return stored as State;
    }
  } catch {
    // Storage can be unavailable (private mode, blocked cookies). The toggle
    // still works for the session; it just does not persist.
  }
  return "system";
}

export default function ThemeToggle() {
  // Starts at the server-rendered value and corrects on mount, so the markup
  // the server sent and the first client render agree.
  const [state, setState] = useState<State>("system");

  useEffect(() => setState(read()), []);

  useEffect(() => {
    const root = document.documentElement;
    if (state === "system") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", state);
    try {
      localStorage.setItem("axio-theme", state);
    } catch {
      // See read(); persistence is best-effort.
    }
  }, [state]);

  return (
    <button
      type="button"
      className="theme-toggle"
      aria-live="polite"
      onClick={() => setState(STATES[(STATES.indexOf(state) + 1) % STATES.length])}
    >
      theme: {state}
    </button>
  );
}
