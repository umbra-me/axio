import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

// The canonical origin. Every absolute URL the metadata emits derives from this
// one, so a move needs one edit rather than a grep.
export const SITE = "https://axio.sh";

// The same values globals.css declares as custom properties. Generated images
// are rendered by satori, which never sees the stylesheet, so the tokens have to
// exist here too. Keep them in step by hand — nothing checks it, which is the
// same trade the site already makes against web-stack's Tailwind config.
export const BG = "#050505";
export const FG = "#fafafa";
export const MUTED = "#8c8c8c";
export const ACCENT = "#f59e0b";
export const GRADIENT = "linear-gradient(110deg, #fbbf24 0%, #f59e0b 52%, #8b5cf6 100%)";
/** The wordmark's letter sits on the gradient, so it is dark rather than light. */
export const MARK_INK = "#1a1204";

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
export const geistMono = (weight: "Regular" | "Bold") =>
  font("geist-mono", `GeistMono-${weight}.ttf`);
