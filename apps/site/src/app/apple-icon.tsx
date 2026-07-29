import { ImageResponse } from "next/og";
import { BG, GRADIENT, MARK_INK, geistMono } from "./brand";

// Home-screen icons are composited onto whatever wallpaper is behind them, and
// iOS applies its own mask, so this one keeps the ground rather than bleeding
// the gradient to the edge: the mark stays a mark instead of becoming the tile.
export const size = { width: 180, height: 180 };
export const contentType = "image/png";

export default function AppleIcon() {
  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: BG,
        }}
      >
        <div
          style={{
            width: 124,
            height: 124,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            backgroundImage: GRADIENT,
            borderRadius: 30,
            color: MARK_INK,
            fontFamily: "Geist Mono",
            fontSize: 78,
            fontWeight: 700,
            paddingLeft: 4,
          }}
        >
          a
        </div>
      </div>
    ),
    { ...size, fonts: [{ name: "Geist Mono", data: geistMono("Bold"), weight: 700, style: "normal" }] },
  );
}
