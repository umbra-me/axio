import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // The container runs `node server.js` rather than `next start`, so the build
  // has to emit its own dependency tree.
  output: "standalone",
  // This app is a leaf inside a Rust workspace. Without an explicit root Next
  // walks up looking for a lockfile, finds Cargo.lock's directory instead, and
  // traces the whole repository.
  turbopack: { root: process.cwd() },
  outputFileTracingRoot: process.cwd(),
};

export default nextConfig;
