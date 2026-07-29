import type { Metadata, Viewport } from "next";
import { GeistSans } from "geist/font/sans";
import { GeistMono } from "geist/font/mono";
import { BG, SITE } from "./brand";
import "./globals.css";

const TITLE = "axio — an AI coding agent that stays in your terminal";
const DESCRIPTION =
  "A cross-platform AI coding agent in Rust: a one-shot CLI and an inline terminal interface. Pre-release, Apache-2.0.";

// Geist is the Umbra house face. next/font self-hosts it, so the page still
// makes no external request.
//
// metadataBase is what makes the og:image absolute. Without it Next emits the
// generated card as a relative path, every scraper that reads it resolves the
// path against its own host, and the unfurl arrives with no image and no error
// anywhere to say why.
export const metadata: Metadata = {
  metadataBase: new URL(SITE),
  title: TITLE,
  description: DESCRIPTION,
  applicationName: "axio",
  authors: [{ name: "Umbra", url: "https://umbra.me" }],
  alternates: { canonical: "/" },
  openGraph: {
    type: "website",
    url: SITE,
    siteName: "axio",
    title: TITLE,
    description: DESCRIPTION,
    locale: "en_US",
  },
  // The card and its alt text come from opengraph-image.tsx by file
  // convention; Next reuses them here, so naming the image twice would only
  // create somewhere for the two to disagree.
  twitter: { card: "summary_large_image", title: TITLE, description: DESCRIPTION },
};

// The site has one palette and no light variant. Declaring that stops the
// browser rendering scrollbars, form controls and the address bar in light
// chrome around a page that is #050505 throughout.
export const viewport: Viewport = {
  themeColor: BG,
  colorScheme: "dark",
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
