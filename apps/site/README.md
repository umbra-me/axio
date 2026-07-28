# apps/site

The axio.sh website, and the install scripts it serves. Next.js 16, App Router,
one page.

```sh
cd apps/site
npm install
npm run dev        # http://localhost:3311
npm run build      # production build
npm run typecheck
```

Port 3311 is the host port the deployed container maps to. It sits between 3310
and 3312, which the retired Axio cloud used and which are now free.

## A Node app inside a Rust workspace

This directory is a leaf. It shares no lockfile, tsconfig or build with the
crates above it, and it is not a Cargo workspace member — `scripts/limits.sh`
counts members and Rust lines under `crates/`, so it does not see anything here.
`scripts/firewall.sh` greps the whole tracked tree, so everything here,
`package-lock.json` included, is subject to it like any other file.

Two consequences worth knowing:

- `next.config.ts` pins `outputFileTracingRoot` and the turbopack root to this
  directory. Without it Next walks up looking for a lockfile, finds the one
  beside `Cargo.lock`, and traces the whole repository into the build.
- `Dockerfile` takes `apps/site` as its build context, not the repository root,
  so the crates never enter the image or its layer cache.

## Design

Built in the Umbra design language so the site reads as one of the family. The
tokens come from `products/web-stack/packages/ui/tailwind.config.ts` in the
Umbra control plane rather than from eyeballing a screenshot: `#050505` ground,
`#18181b` surfaces, `#262626` hairlines, `0.5rem` radius, Geist, blurred colour
fields behind the fold, pill section labels, a gradient phrase in the headline,
a stat strip, bordered cards.

What makes it axio rather than Umbra is the accent, using a mechanism that
design system already has: every product carries its own hue inside the shared
dark shell — aegis emerald, hermes violet, nebula pink, nexus indigo. axio takes
amber, the colour a terminal has historically been and the only warm note
against an otherwise cold palette, and its gradient resolves from that amber
into the house violet.

The ambient texture behind the hero is axio's too. umbra.me drifts a
constellation network; this drifts scrollback, rendered from the product's own
output rather than filler, and masked away from the centre so it reads as
material and never as content.

Geist is self-hosted through `next/font`. The page makes no external request.

## Install scripts

`scripts/install.sh` and `scripts/install.ps1` are served at `/install` and
`/install.ps1` by route handlers that read them from disk at build time. They
are real files rather than strings inside a module, so they stay shellcheck-able
and readable here.

Both are served as `text/plain` with `nosniff`, so a browser shows the source
instead of downloading it — anyone about to pipe a script into their shell
should be able to read it first.

Both build from source with cargo, because there is nothing else to do: no
release is tagged, so there is no binary to download and no checksum to verify.
They are shaped so a released-binary path can be added ahead of the source build
when that changes.

Two things in them are easy to get wrong and expensive to find later:

- The version check compares major and minor **numerically**. A string compare
  ranks `1.100` below `1.88`, so a naive check would start rejecting every
  toolchain the day Rust reaches 1.100.
- Success is reported from the binary that was just installed, not from whatever
  `axio` resolves to. An older copy earlier on `PATH` would otherwise let the
  script confirm a build that never took effect.

## Content rule

Every claim on the page is either in the repository README or in the code. Where
the two disagreed, the code won — `crates/axio-core/src/auth.rs` declares four
providers, and the README said three until it was corrected.

The verification table is the part most likely to go stale, because it records
what has been run against a live endpoint rather than what exists. Update it when
a transport meets its endpoint or a release is cut. A page claiming a path is
unverified after it has been verified is a smaller problem than the reverse, but
both are wrong.

## Deployment

Deployed by the Umbra control plane as the `axio-site` stack:
`infra/deploy/axio-site.yaml`, compose project `site-axio-site`, container
`umbra-axio-site`, host port 3311. It builds to `output: "standalone"` and runs
`node server.js` on port 3000 inside the container.
