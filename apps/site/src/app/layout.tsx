import type { Metadata } from "next";
import { GeistSans } from "geist/font/sans";
import { GeistMono } from "geist/font/mono";
import "./globals.css";

// Geist is the Umbra house face. next/font self-hosts it, so the page still
// makes no external request.
export const metadata: Metadata = {
  title: "axio — an AI coding agent that stays in your terminal",
  description:
    "A cross-platform AI coding agent in Rust: a one-shot CLI and an inline terminal interface. Pre-release, Apache-2.0.",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html
      lang="en"
      className={`${GeistSans.variable} ${GeistMono.variable}`}
    >
      <body>{children}</body>
    </html>
  );
}
