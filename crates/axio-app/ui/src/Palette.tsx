import { useEffect, useMemo, useRef, useState } from "react";
import { IconClose } from "./icons";

// The command palette: everything the window can do, by name, from the keyboard.
//
// Given the list rather than deciding it. Which sessions exist and which
// harnesses are installed is the window's knowledge, not this component's;
// this only filters, ranks and runs.
export type Command = {
  id: string;
  title: string;
  /** Shown beside the title: a repository, a chord, a status. */
  detail?: string;
  /** A short group label; commands are shown in group order. */
  group: string;
  run: () => void;
};

function score(query: string, command: Command): number {
  const q = query.trim().toLowerCase();
  if (q === "") return 1;
  const hay = `${command.title} ${command.detail ?? ""} ${command.group}`.toLowerCase();
  if (hay.startsWith(q)) return 3;
  if (hay.includes(q)) return 2;
  // Every character in order — the fuzzy match a fast typist relies on.
  let at = 0;
  for (const ch of q) {
    at = hay.indexOf(ch, at);
    if (at < 0) return 0;
    at += 1;
  }
  return 1;
}

export function Palette({ commands, onClose }: { commands: Command[]; onClose: () => void }) {
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const input = useRef<HTMLInputElement>(null);

  const shown = useMemo(() => {
    const ranked = commands
      .map((c) => ({ c, s: score(query, c) }))
      .filter(({ s }) => s > 0)
      .sort((a, b) => b.s - a.s);
    return ranked.map(({ c }) => c).slice(0, 40);
  }, [commands, query]);

  useEffect(() => {
    input.current?.focus();
  }, []);

  useEffect(() => {
    setCursor(0);
  }, [query]);

  const run = (command: Command | undefined) => {
    if (!command) return;
    onClose();
    command.run();
  };

  return (
    <div className="scrim top" onMouseDown={onClose}>
      <div
        className="palette"
        role="dialog"
        aria-label="Commands"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="palette-input">
          <input
            ref={input}
            value={query}
            placeholder="Type a command, a session, or a repository…"
            aria-label="Search commands"
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setCursor((c) => Math.min(c + 1, shown.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setCursor((c) => Math.max(c - 1, 0));
              } else if (e.key === "Enter") {
                e.preventDefault();
                run(shown[cursor]);
              } else if (e.key === "Escape") {
                e.preventDefault();
                onClose();
              }
            }}
          />
          <button onClick={onClose} aria-label="Close">
            <IconClose size={12} />
          </button>
        </div>
        <ul className="palette-list" role="listbox">
          {shown.length === 0 && <li className="palette-empty">Nothing matches.</li>}
          {shown.map((command, n) => {
            const first = n === 0 || shown[n - 1]?.group !== command.group;
            return (
              <li
                key={command.id}
                role="option"
                aria-selected={n === cursor}
                className={n === cursor ? "on" : undefined}
                data-group={first ? command.group : undefined}
                onMouseEnter={() => setCursor(n)}
                onClick={() => run(command)}
              >
                <span className="title">{command.title}</span>
                {command.detail && <span className="detail">{command.detail}</span>}
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}
