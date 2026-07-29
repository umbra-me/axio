import { ImageResponse } from "next/og";
import {
  ACCENT,
  BG,
  FG,
  GRADIENT,
  MARK_INK,
  MUTED,
  geistMono,
  geistSans,
} from "./brand";

// The card a pasted link unfurls into. It carries the same three things the
// hero does — the mark, the headline, the command — because someone who sees
// this and never clicks should still know what axio is and how it is installed.
export const alt = "axio — an AI coding agent that stays in your terminal";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

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
          // The hero's blurred colour field, flattened to two radial stops.
          // satori has no filter: blur(), so the softness is in the gradient.
          backgroundImage:
            "radial-gradient(900px 460px at 12% -12%, rgba(245,158,11,0.20), rgba(5,5,5,0) 62%)," +
            "radial-gradient(760px 420px at 96% 108%, rgba(124,58,237,0.18), rgba(5,5,5,0) 60%)",
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
                backgroundImage: GRADIENT,
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
            <div style={{ marginLeft: 18, fontSize: 34, fontWeight: 600, letterSpacing: "-0.02em" }}>
              axio
            </div>
          </div>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              border: "1px solid #262626",
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
                background: ACCENT,
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
              // `.hero h1` in globals.css, at the top of its clamp: 700 at
              // -0.045em over 1.03. The card is the page's opening line, so it
              // is set the way the page sets it rather than approximately.
              fontSize: 78,
              fontWeight: 700,
              letterSpacing: "-0.045em",
              lineHeight: 1.03,
            }}
          >
            <div style={{ display: "flex" }}>It stays in your&nbsp;</div>
            <div
              style={{
                display: "flex",
                backgroundImage: GRADIENT,
                backgroundClip: "text",
                color: "transparent",
              }}
            >
              terminal.
            </div>
          </div>
          <div style={{ marginTop: 26, fontSize: 29, color: MUTED, lineHeight: 1.45, maxWidth: 880 }}>
            A cross-platform AI coding agent in Rust. One binary, two surfaces —
            a one-shot CLI when you pipe into it, an inline composer when you don&apos;t.
          </div>
        </div>

        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              background: "#101012",
              border: "1px solid #262626",
              borderRadius: 8,
              padding: "18px 26px",
              fontFamily: "Geist Mono",
              fontSize: 25,
            }}
          >
            <div style={{ color: ACCENT, marginRight: 14 }}>$</div>
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
        { name: "Geist", data: geistSans("Bold"), weight: 700, style: "normal" },
        { name: "Geist Mono", data: geistMono("Regular"), weight: 400, style: "normal" },
        { name: "Geist Mono", data: geistMono("Bold"), weight: 700, style: "normal" },
      ],
    },
  );
}
