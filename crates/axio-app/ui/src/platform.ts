// Which desktop this window is on.
//
// One question, asked once. On macOS the window keeps its native controls —
// the traffic lights, placed by `tauri.macos.conf.json` inside the custom
// title bar — so the drawn ones are hidden and the wordmark moves right to
// make room. Everywhere else the frame is ours.
export const isMac: boolean =
  typeof navigator !== "undefined" &&
  (navigator.platform.startsWith("Mac") || /Mac OS X/.test(navigator.userAgent));
