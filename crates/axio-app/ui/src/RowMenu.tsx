import { useEffect, useRef, useState } from "react";

// A small menu for one row: right-click or the ⋯ at the row's end.
//
// Items are given, never guessed — a session's menu and a terminal's differ,
// and the rail decides which is which. Rename is the one item with a second
// step; it is handled here so both callers get the same inline field.
export type MenuItem =
  | { kind: "action"; title: string; danger?: boolean; run: () => void }
  | { kind: "rename"; title: string; current: string; run: (title: string | null) => void }
  | { kind: "rule" };

export function RowMenu({
  items,
  at,
  onClose,
}: {
  items: MenuItem[];
  /** Where it opens, in viewport pixels. */
  at: { x: number; y: number };
  onClose: () => void;
}) {
  const host = useRef<HTMLElement | null>(null);
  const hold = (el: HTMLElement | null) => {
    host.current = el;
  };
  const [renaming, setRenaming] = useState<Extract<MenuItem, { kind: "rename" }> | null>(null);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (host.current && !host.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  // Keep it on screen: open upward or leftward when it would not fit.
  const style: React.CSSProperties = {
    left: Math.min(at.x, window.innerWidth - 240),
    top: Math.min(at.y, window.innerHeight - 40 * items.length - 24),
  };

  if (renaming) {
    return (
      <div className="menu row-menu" style={style} ref={hold} role="dialog" aria-label={renaming.title}>
        <input
          autoFocus
          className="rename"
          value={draft}
          placeholder={renaming.current}
          aria-label="New name"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              renaming.run(draft.trim() === "" ? null : draft.trim());
              onClose();
            }
          }}
        />
        <span className="menu-detail">Enter keeps it · empty clears the name</span>
      </div>
    );
  }

  return (
    <ul className="menu row-menu" style={style} ref={hold} role="menu">
      {items.map((item, n) =>
        item.kind === "rule" ? (
          <li className="menu-rule" role="separator" key={n} />
        ) : (
          <li role="none" key={n}>
            <button
              role="menuitem"
              className={item.kind === "action" && item.danger ? "danger" : undefined}
              onClick={() => {
                if (item.kind === "rename") {
                  setDraft(item.current);
                  setRenaming(item);
                  return;
                }
                item.run();
                onClose();
              }}
            >
              <span className="menu-title">{item.title}</span>
            </button>
          </li>
        ),
      )}
    </ul>
  );
}

/** Copy to the clipboard, quietly. The webview has the API; nothing else is needed. */
export function copy(text: string) {
  void navigator.clipboard?.writeText(text).catch(() => {});
}
