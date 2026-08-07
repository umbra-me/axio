import { ImageResponse } from "next/og";
import {
  ACCENT,
  BG,
  CLAUDE,
  CODEX,
  FG,
  MARK_INK,
  MUTED,
  PI,
  SLATE,
  WARN,
  geistMono,
  geistSans,
} from "./brand";

// The card a pasted link unfurls into. It carries the same three things the
// hero does — the mark, the headline, the command — because someone who sees
// this and never clicks should still know what axio is and how it is installed.
//
// The fourth thing is the agent palette, as four dots on the install slab. It
// is the one part of the identity that cannot be said in a sentence at this
// size, and it is what makes the card recognisably this product rather than
// another dark card with a monospaced headline.
export const alt = "axio — a local-first AI coding agent in Rust";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

const DOTS = [ACCENT, CLAUDE, CODEX, PI];

export default function OpengraphImage() {
  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          justifyContent: "space-between",
          background: BG,
          // The hero's single cold field, flattened to one radial stop. satori
          // has no filter: blur(), so the softness lives in the gradient.
          backgroundImage:
            "radial-gradient(920px 520px at 88% -18%, rgba(123,160,255,0.20), rgba(5,7,10,0) 64%)",
          padding: 72,
          fontFamily: "Geist",
          color: FG,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
          <div style={{ display: "flex", alignItems: "center" }}>
            <div
              style={{
                width: 46,
                height: 46,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                background: ACCENT,
                borderRadius: 11,
                color: MARK_INK,
                fontFamily: "Geist Mono",
                fontSize: 27,
                fontWeight: 700,
                paddingLeft: 2,
              }}
            >
              a
            </div>
            <div
              style={{
                marginLeft: 18,
                fontFamily: "Geist Mono",
                fontSize: 32,
                fontWeight: 600,
                letterSpacing: "-0.02em",
              }}
            >
              axio
            </div>
          </div>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              border: "1px solid rgba(255,255,255,0.10)",
              borderRadius: 999,
              padding: "10px 22px",
              fontSize: 20,
              color: MUTED,
            }}
          >
            <div
              style={{
                width: 8,
                height: 8,
                borderRadius: 999,
                background: WARN,
                marginRight: 11,
              }}
            />
            Pre-release · nothing tagged
          </div>
        </div>

        <div style={{ display: "flex", flexDirection: "column" }}>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              // `.hero h1` in globals.css, at the top of its clamp: Geist Mono
              // at 600, -0.035em over 1.06. The card is the page's opening line,
              // so it is set the way the page sets it rather than approximately.
              fontFamily: "Geist Mono",
              fontSize: 72,
              fontWeight: 600,
              letterSpacing: "-0.035em",
              lineHeight: 1.06,
            }}
          >
            <div style={{ display: "flex" }}>Many agents.</div>
            <div style={{ display: "flex", color: ACCENT }}>One queue.</div>
          </div>
          <div style={{ marginTop: 28, fontSize: 27, color: MUTED, lineHeight: 1.45, maxWidth: 830 }}>
            A local-first coding agent in Rust. Every supervised session gets its
            own git worktree and branch, and every approval they need arrives in
            one place.
          </div>
        </div>

        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              background: SLATE,
              border: "1px solid rgba(255,255,255,0.10)",
              borderRadius: 10,
              padding: "18px 26px",
              fontFamily: "Geist Mono",
              fontSize: 24,
            }}
          >
            <div style={{ display: "flex", marginRight: 20 }}>
              {DOTS.map((color) => (
                <div
                  key={color}
                  style={{
                    width: 9,
                    height: 9,
                    borderRadius: 999,
                    background: color,
                    marginRight: 7,
                  }}
                />
              ))}
            </div>
            curl -fsSL https://axio.sh/install | sh
          </div>
          <div style={{ fontSize: 21, color: MUTED }}>Apache-2.0 · a product of Umbra</div>
        </div>
      </div>
    ),
    {
      ...size,
      fonts: [
        { name: "Geist", data: geistSans("Regular"), weight: 400, style: "normal" },
        { name: "Geist", data: geistSans("SemiBold"), weight: 600, style: "normal" },
        { name: "Geist Mono", data: geistMono("Regular"), weight: 400, style: "normal" },
        { name: "Geist Mono", data: geistMono("SemiBold"), weight: 600, style: "normal" },
        { name: "Geist Mono", data: geistMono("Bold"), weight: 700, style: "normal" },
      ],
    },
  );
}
