import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

// The canonical origin. Every absolute URL the metadata emits derives from this
// one, so a move needs one edit rather than a grep.
export const SITE = "https://axio.sh";

// The same values globals.css declares as custom properties. Generated images
// are rendered by satori, which never sees the stylesheet, so the tokens have to
// exist here too. Keep them in step by hand — nothing checks it, which is the
// same trade the site already makes against web-stack's Tailwind config.
//
// They are the desktop application's, from crates/axio-app/ui/src/styles/
// tokens.css, rather than the house palette this site opened with. One product
// with two surfaces cannot credibly wear two identities, and the application is
// the one with a stated visual thesis to inherit: the chrome is glass and the
// content is slate.
export const BG = "#05070a";
export const SLATE = "#0a0d13";
export const LINE = "rgba(255,255,255,0.10)";
export const FG = "#f4f5f7";
export const TEXT_2 = "#bfc3cb";
export const MUTED = "#8f939d";
export const FAINT = "#63666f";

/** axio's own. Focus rings, the mark, anything belonging to the application. */
export const ACCENT = "#7ba0ff";
/** The hosted agents, each with its own. A session never borrows the chrome's. */
export const CLAUDE = "#b492fd";
export const CODEX = "#55dcb0";
export const PI = "#63cbdc";

export const OK = "#55dcb0";
export const WARN = "#e2c67e";

/** The wordmark's letter sits on the accent, so it is the ground rather than light. */
export const MARK_INK = "#05070a";

// satori cannot read woff2, and geist ships both — these are the .ttf twins of
// the faces next/font serves to the browser, so the generated images render in
// the same typeface the page does rather than in a fallback.
//
// Found by walking up from the working directory rather than by resolving the
// package: geist's `exports` map publishes only ./font/*, so neither the .ttf
// files nor its own package.json can be resolved by specifier — and webpack
// rewrites a `require.resolve` in this file into one of its own numeric module
// ids, which fails at build with a type error rather than a missing file.
// Walking up also survives a hoisted install, which a path anchored at
// process.cwd() alone would not.
const SUFFIX = path.join("node_modules", "geist", "dist", "fonts");

function fontDir(): string {
  let dir = process.cwd();
  for (;;) {
    const candidate = path.join(dir, SUFFIX);
    if (existsSync(candidate)) return candidate;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  // Reached only if the dependency is absent, in which case failing here with
  // the path that was searched beats emitting a card in a fallback face.
  throw new Error(`geist fonts not found: no ${SUFFIX} above ${process.cwd()}`);
}

function font(family: "geist-sans" | "geist-mono", file: string): ArrayBuffer {
  const buf = readFileSync(path.join(fontDir(), family, file));
  return buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength) as ArrayBuffer;
}

export const geistSans = (weight: "Regular" | "SemiBold" | "Bold") =>
  font("geist-sans", `Geist-${weight}.ttf`);
export const geistMono = (weight: "Regular" | "SemiBold" | "Bold") =>
  font("geist-mono", `GeistMono-${weight}.ttf`);
