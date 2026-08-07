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

Umbra manages this website's portfolio record, health, analytics identity, and
public configuration through `admin.umbra.me`. The site also exposes a scoped,
read-only Product Admin adapter that reports the current local-first product and
website boundary. It does not revive hosted accounts or the retired Axio cloud,
and there is no standalone Axio admin site.

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

The desktop application's visual system, ported rather than paraphrased.
`crates/axio-app/ui/src/styles/tokens.css` states the thesis — **the chrome is
glass and the content is slate** — and this page is built on it. The header,
the section labels and the panel the headline sits in are translucent and
float; the parts you actually read — a transcript, the verification ledger, an
install command — are dense near-opaque slabs sitting on top of the page rather
than in it.

In the application the glass is translucent to your desktop. A web page has no
wallpaper, so the hero supplies its own: `Surface.tsx` draws a slab of the
application's own surface — four sessions across two repositories, each wearing
its agent's colour, one of them holding a question nobody has answered yet —
and the headline sits in a glass panel lapping over its left edge.

**The lap is 1rem, and that is a ceiling rather than a taste call.** The rail
begins at the slab's left edge and the first session dot sits about 28px in, so
a larger lap hides the four agent colours — the one thing in the hero that
cannot afford to be behind anything. An earlier 128px lap buried three of them.
Depth is carried by the panel's drop shadow and by the slab running off the
right edge of the frame instead.

The palette is the application's rather than the house one this site opened
with: `#05070a` ground, `#0a0d13` slabs, `#7ba0ff` for axio itself, and one
colour for each agent it can host — claude violet, codex green, pi cyan. That
is a system rather than an accent, and it is what keeps "this belongs to an
agent" and "this belongs to axio" different questions on the page, as they are
in the window. The amber and its gradient are gone: one product with two
surfaces cannot credibly wear two identities, and the application has no
gradient anywhere.

The display face is Geist Mono, not Geist Sans. The usual pairing runs a sans
display over mono captions; this product's entire material is monospaced
output, and the application's UI face deliberately stops at 30px because it is
a tool rather than a poster. The cost is real and the headline pays it: Geist
Mono's advance is roughly twice Geist Sans's, so a display line here buys about
twelve characters.

Structure carries content rather than decorating it. The strip under the hero
is the application's status bar — the count is the fact and the noun beside it
is the caption, so they are not the same colour, and the one figure that is not
neutral does not read as neutral. Each behaviour card is marked by a string the
product actually prints (`exit 5`, `--probe`, `~/.axio`) rather than by a
tinted square with a glyph in it, which would have meant nothing.

Geist is self-hosted through `next/font`. The page makes no external request.

Two affordances rather than decoration. Each install block carries a copy button
in its terminal bar — in the bar so it never covers the command it copies and
needs no hover to be found. It renders only after mount and only where the
Clipboard API exists, because that API is absent on plain http and in older
browsers and a button that silently does nothing is worse than none; the command
is still there to select. A skip link precedes the header, and its target takes
`tabindex="-1"` so focus actually moves rather than the page merely scrolling.

Motion is one curve — the application's `cubic-bezier(0.16, 1, 0.3, 1)` — and
very little uses it. The authored moment is ported from the application
unchanged: the selected session's accent wipes down its leading edge as the
page settles, and a running session's dot breathes. Nothing else animates
position or glow, and the drifting scrollback the hero used to carry is gone —
with a real slab behind the glass there was already material back there, and
two ambient textures competed for the same space.

Motion is opt-out throughout, through one mechanism rather than a habit each
rule has to remember: `--fast`, `--base` and `--slow` collapse to `1ms` under
`prefers-reduced-motion: reduce`, the wipe and the breathing stop outright, the
caret hides, and `HeroTranscript` checks the same query in JS and jumps
straight to its settled state.

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

The mark is Geist Mono's `a` on the accent. It is drawn rather than
hand-authored as an SVG path because an SVG favicon would have to name a font
the browser has no reason to have, and would fall back to whatever the platform
calls monospace.

The header wordmark is **not** the same object any more. It is the
application's own mark — a small filled square at the accent with a short
bloom, the same primitive its running-session dot uses, so one shape does
identity and state. A letterless square is right beside the word `axio` and
wrong in a 32×32 tab, which is why the icons keep the glyph.

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
headline is set the way `.hero h1` sets it — Geist Mono at 600, `-0.035em` over
1.06 — rather than approximately, and its second line takes the accent as the
page's does. It carries one thing the page states in words instead: the four
agent colours, as dots on the install slab. At this size that palette is the
part of the identity a sentence cannot deliver, and it is what makes the card
recognisably this product rather than another dark card with a monospaced
headline.

If the headline or its treatment changes, change it in both places — the card
is meant to match the page, not improve on it.

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
