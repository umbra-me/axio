// One bundle, two surfaces.
//
// The window and the flyout are separate Tauri windows over the same `index.html`; the
// hash decides which renders. Two Vite entry points would mean two builds of the same
// component tree for no gain.

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { enableWindowDrag } from "./drag";
import { Flyout } from "./Flyout";
import { Window } from "./Window";
import "./styles.css";

const isFlyout = window.location.hash === "#flyout";

// The flyout is positioned against the tray icon and dismissed on blur, so moving it
// would only ever detach it from the thing it points at. Only the window drags.
if (!isFlyout) enableWindowDrag();

createRoot(document.getElementById("root")!).render(
  <StrictMode>{isFlyout ? <Flyout /> : <Window />}</StrictMode>,
);
