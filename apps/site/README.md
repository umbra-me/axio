# apps/site

The axio website. Next.js 16, App Router, one page.

```sh
cd apps/site
npm install
npm run dev        # http://localhost:3311
npm run build      # production build
npm run typecheck
```

Port 3311 sits between the two hosts the legacy Axio deployment uses, 3310 and
3312, so it does not collide with either while both are still running.

## A Node app inside a Rust workspace

This directory is a leaf. It shares no lockfile, tsconfig or build with the
crates above it, and it is not a Cargo workspace member — `scripts/limits.sh`
counts members and Rust LOC under `crates/`, so it does not see anything here.

Two consequences worth knowing:

- `next.config.ts` pins `outputFileTracingRoot` and the turbopack root to this
  directory. Without that Next walks up looking for a lockfile, finds the one
  next to `Cargo.lock`, and traces the whole repository into the build.
- `Dockerfile` takes `apps/site` as its build context, not the repository root,
  so the Rust sources never enter the image or its layer cache.

`scripts/firewall.sh` greps the whole tracked tree, so everything here —
including `package-lock.json` — is subject to it like any other file.

## Content rule

Every claim on the page is either in the README at the repository root or in the
code. Where the two disagreed, the code won:

- `crates/axio-core/src/auth.rs:52` declares four providers. The root README's
  Providers section still says "three names over two implementations", written
  before `openai-codex` landed. The page says four names over three dialects.

The verification table is the part most likely to go stale, because it records
what has been run against a live endpoint rather than what exists. Update it
when a transport meets its endpoint or a release is cut. A page that claims a
path is unverified after it has been verified is a smaller problem than the
reverse, but both are wrong.

## Design notes

The page is light and the terminal blocks are dark, and in dark mode the page
inverts while those blocks do not. That is the product's own behaviour: axio
renders in the terminal's own colours rather than a bundled theme, so the one
element standing for it looks the same whatever surrounds it. The dark page
ground is deliberately a clear step lighter than the block — when they were
within a few points the block disappeared.

No webfonts. A webfont would be the only network request the page makes.

## Deployment

Not wired up yet. It builds to `output: "standalone"` and runs as
`node server.js` on port 3000 inside the container, mapped to 3311 on the host —
the same shape as the other per-site projects in the Umbra control plane.
