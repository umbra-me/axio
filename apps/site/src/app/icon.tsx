import { ImageResponse } from "next/og";
import { ACCENT, MARK_INK, geistMono } from "./brand";

// The header wordmark's mark, rendered at build time so the tab shows the same
// object the page opens with. Drawn rather than hand-authored as an SVG path
// because the glyph is Geist Mono's `a` — an SVG favicon would have to name a
// font the browser has no reason to have, and would fall back to whatever the
// platform calls monospace.
export const size = { width: 32, height: 32 };
export const contentType = "image/png";

export default function Icon() {
  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: ACCENT,
          borderRadius: 7,
          color: MARK_INK,
          fontFamily: "Geist Mono",
          fontSize: 21,
          fontWeight: 700,
          // The glyph's own sidebearing sits it left of centre at this size.
          paddingLeft: 1,
        }}
      >
        a
      </div>
    ),
    { ...size, fonts: [{ name: "Geist Mono", data: geistMono("Bold"), weight: 700, style: "normal" }] },
  );
}
