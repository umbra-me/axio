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

Two affordances rather than decoration. Each install block carries a copy button
in its terminal bar — in the bar so it never covers the command it copies and
needs no hover to be found. It renders only after mount and only where the
Clipboard API exists, because that API is absent on plain http and in older
browsers and a button that silently does nothing is worse than none; the command
is still there to select. A skip link precedes the header, and its target takes
`tabindex="-1"` so focus actually moves rather than the page merely scrolling.

Motion is opt-out throughout: the drifting scrollback sits behind a
`prefers-reduced-motion: no-preference` query, the caret hides under `reduce`,
and `HeroTranscript` checks the same query in JS and jumps straight to its
settled state.

## Metadata and generated images

`layout.tsx` sets `metadataBase`, a canonical URL, Open Graph and card metadata.
`metadataBase` is the load-bearing one: without it Next emits `og:image` as a
relative path, every scraper resolves that against its own host, and the unfurl
arrives with no image and nothing anywhere to say why.

Four images are generated at build time rather than committed. No binary asset
lives in this repository and no dependency was added for them.

| File | Emits | Is |
| --- | --- | --- |
| `icon.tsx` | `/icon` | 32×32 tab icon |
| `apple-icon.tsx` | `/apple-icon` | 180×180 home-screen icon, on the ground rather than bleeding to the edge |
| `opengraph-image.tsx` | `/opengraph-image` | the 1200×630 card, reused for `twitter:image` |
| `robots.ts`, `sitemap.ts` | `/robots.txt`, `/sitemap.xml` | — |

The mark is Geist Mono's `a` on the product gradient, which is what the header
wordmark is. It is drawn rather than hand-authored as an SVG path because an SVG
favicon would have to name a font the browser has no reason to have, and would
fall back to whatever the platform calls monospace.

`brand.ts` holds the tokens and loads the fonts. Three things there are easy to
get wrong:

- **The renderer never sees the stylesheet.** The tokens are restated in
  `brand.ts`, and nothing checks that they still match `globals.css`. This is the
  same trade the site already makes against web-stack's Tailwind config, and it
  drifts the same way.
- **woff2 is unreadable to the image renderer**, and geist ships both formats.
  `brand.ts` loads the `.ttf` twins of the faces `next/font` serves the browser,
  so the images render in the same typeface the page does rather than a fallback.
- **The fonts are found by walking up for `node_modules/geist/dist/fonts`, not
  by resolving the package.** geist's `exports` map publishes only `./font/*`, so
  neither the `.ttf` files nor its own `package.json` can be resolved by
  specifier — and a `require.resolve` here is rewritten by webpack into one of
  its own numeric module ids, which fails the build with a type error rather than
  a missing file. Walking up also survives a hoisted install.

`robots.ts` disallows `/install` and `/install.ps1`. They stay reachable — a
shell still fetches them — but a crawler that indexes them turns a search result
into a page whose entire content is a script.

The card repeats the mark, the headline and the install command, so someone who
sees it and never clicks still knows what axio is and how to install it. Its
headline is set the way `.hero h1` sets it, 700 at -0.045em over 1.03, rather
than approximately. The gradient word fades toward pale violet at its tail; the
page does that too, because `.gradient-text` puts the gradient box around the
word itself. If that is ever changed, change it in both places — the card is
meant to match the page, not improve on it.

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
