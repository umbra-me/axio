// The keyboard, in one place.
//
// One listener on the window, one table of chords, and every chord is the
// platform's own modifier — Command on macOS, Control elsewhere — so the
// bindings read the way the rest of the desktop does. Chords are named here
// and *only* here, which is what lets the palette list them without a second
// copy drifting.

import { isMac } from "./platform";

export type Chord = {
  key: string;
  shift?: boolean;
  alt?: boolean;
  /** How the chord is written in a menu. */
  label: string;
};

export const chords = {
  newSession: { key: "n", label: "N" },
  newTerminal: { key: "t", shift: true, label: "⇧T" },
  palette: { key: "k", label: "K" },
  closeTab: { key: "w", label: "W" },
  split: { key: "\\", label: "\\" },
  rail: { key: "b", label: "B" },
  nextTab: { key: "]", shift: true, label: "⇧]" },
  prevTab: { key: "[", shift: true, label: "⇧[" },
  addRepository: { key: "o", label: "O" },
  settings: { key: ",", label: "," },
  attention: { key: "a", shift: true, label: "⇧A" },
  railPrev: { key: "arrowup", alt: true, label: "⌥↑" },
  railNext: { key: "arrowdown", alt: true, label: "⌥↓" },
} satisfies Record<string, Chord>;

export type Action = keyof typeof chords;

export const modifier = isMac ? "⌘" : "Ctrl+";

export function label(action: Action): string {
  return `${modifier}${chords[action].label}`;
}

/** Which action a key event names, if any. Digits are the tab switcher. */
export function actionFor(e: KeyboardEvent): Action | { tab: number } | null {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (!mod) return null;
  if (/^[1-9]$/.test(e.key) && !e.shiftKey && !e.altKey) return { tab: Number(e.key) - 1 };
  const key = e.key.toLowerCase();
  for (const [name, chord] of Object.entries(chords) as [Action, Chord][]) {
    if (chord.key === key && Boolean(chord.shift) === e.shiftKey && Boolean(chord.alt) === e.altKey) {
      return name;
    }
  }
  return null;
}
