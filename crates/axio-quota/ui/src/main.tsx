// One bundle, two surfaces.
//
// The window and the flyout are separate Tauri windows over the same `index.html`; the
// hash decides which renders. Two Vite entry points would mean two builds of the same
// component tree for no gain.

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Flyout } from "./Flyout";
import { Window } from "./Window";
import "./styles.css";

const isFlyout = window.location.hash === "#flyout";

createRoot(document.getElementById("root")!).render(
  <StrictMode>{isFlyout ? <Flyout /> : <Window />}</StrictMode>,
);
