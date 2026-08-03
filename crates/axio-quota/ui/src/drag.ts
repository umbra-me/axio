// Dragging a frameless window.
//
// `decorations(false)` removes the OS titlebar, and with it the only thing that could move
// the window. Tauri's `data-tauri-drag-region` attribute restores that, but only for the
// exact elements carrying it — which turns "where can I grab this window" into a list
// somebody has to keep updating as the layout changes.
//
// So the rule is inverted here: everything drags except the things that do something when
// clicked. One handler, one list of exceptions, and no attribute to forget on the next
// element somebody adds.
//
// Two things this depends on that are easy to miss:
//
//   * `core:window:allow-start-dragging` must be granted in `capabilities/`. Without it
//     the call is denied and the window simply does not move — no error, no warning. That
//     was the original bug: the attribute was on the titlebar and correct, and the app had
//     no capabilities file at all.
//   * Rust-side calls bypass permissions entirely, which is why the minimise and close
//     buttons worked while dragging did not. The two failures look identical from the
//     outside and have nothing to do with each other.

import { getCurrentWindow } from "@tauri-apps/api/window";

/// Anything that responds to a click, and so must not become a drag handle.
///
/// `[data-no-drag]` is the escape hatch for a future element that is interactive without
/// being one of these tags.
const INTERACTIVE = [
  "button",
  "a",
  "input",
  "select",
  "textarea",
  "label",
  "[role='button']",
  "[contenteditable]",
  "[data-no-drag]",
].join(",");

/// Start moving the window when the press lands on inert chrome.
export function enableWindowDrag() {
  document.addEventListener("mousedown", (event) => {
    // Primary button only. A right-click is a context menu and a middle-click is a paste
    // on some platforms; neither should move the window.
    if (event.button !== 0) return;

    const target = event.target as Element | null;
    if (!target || target.closest(INTERACTIVE)) return;

    // Let a real text selection win where one is possible. The app sets `user-select:
    // none` almost everywhere, so this is about the few places that opt back in.
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;

    // Fire and forget: a denied permission or a closed window is not worth a dialog, and
    // an unhandled rejection here would surface in the console on every click.
    void getCurrentWindow().startDragging().catch(() => {});
  });
}
