import type { Metadata } from "next";
import "./globals.css";

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
    <html lang="en" suppressHydrationWarning>
      <head>
        {/* Applied before first paint. Without this the page renders in the
            system scheme and then snaps to the stored one, which flashes the
            whole background — the most visible thing on a page this plain. */}
        <script
          dangerouslySetInnerHTML={{
            __html: `try{var t=localStorage.getItem("axio-theme");if(t&&t!=="system")document.documentElement.setAttribute("data-theme",t)}catch(e){}`,
          }}
        />
      </head>
      <body>{children}</body>
    </html>
  );
}
