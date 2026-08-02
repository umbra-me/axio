# Changelog

Notable changes per release. Dates are the release date; the format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[SemVer](https://semver.org/spec/v2.0.0.html) — with the usual `0.x` caveat that
a minor bump may break things.

## [Unreleased]

### Added

- **`axio cost`, and the `axio-cost` crate behind it.** What the coding agents on
  this machine have spent, read from the session transcripts they already write.
  No network, no credentials. Group with `--by model|client|session|day|workspace`,
  `--json` for scripting, `--diagnose` for what each parser found and skipped.

  Fifteen agents are covered. Three have hand-written parsers — Claude Code,
  Codex and Grok — because each carries knowledge a generic walk cannot infer:
  which of two token figures is cumulative, which repeated events must be
  suppressed, which vendor reports its own cost. The other twelve are rows in
  `sources::catalog`, driven by one table-driven parser, because these formats
  log the same event and differ only in where they put it.

  An agent whose directory is absent reports *not installed*, which is a
  different answer from *recorded nothing* and is what `--diagnose` prints.

  Every token rule in it was verified against real transcripts, and three of them
  contradict what the formats look like at a glance:

  - Codex reports `input_tokens` **including** cached reads. One real turn is
    71,375 input of which 67,456 is cache — billing the reported figure at the
    fresh rate overcharges eighteenfold.
  - Codex's `total_token_usage` is **cumulative for the session**; only
    `last_token_usage` may be summed, and consecutive repeats must be dropped.
    Across the 396 local sessions with usage, the deduplicated sum reproduces the
    session's own final total exactly in 390 and within 1% in 4 more.
  - Claude Code writes `requestId: null` on proxied turns, so the deduplication
    key falls back to the message id alone rather than collapsing every
    null-request line in a file into one row.

  Prices are a compiled-in table with a documented source per vendor, and a
  refresh overlay for the caller to populate. Anthropic's cache rates derive from
  the input price by fixed multipliers (0.1x read, 1.25x and 2x write) and
  OpenAI's tier above 272K input tokens is honoured per request. Grok records what a turn
  cost it, and that figure is preferred over the table wherever it appears — it is the only number here computed by the party doing
  the charging.

  **A model with no known rate is reported unpriced, never as zero**, and no total
  is printed without the share of tokens it accounts for. That rule has teeth: an
  early build summed the one Codex message in 77,525 whose model it knew and
  printed `$0.30` next to the word *Codex*. `Totals` now hands back a `Cost` that
  cannot be formatted without confronting its own coverage.

- **`axio quota`, and a desktop app behind it.** How much of each provider's
  limit is left and when it resets, for Codex, Claude and OpenRouter. It reads
  the credential files those vendors' own CLIs already wrote — never axio's own
  stored credentials — so it cannot sign you in to anything and needs no
  configuration before it works. `--json` emits one object per provider,
  `--diagnose` prints where each probe looks and whether the credential is
  there.

  `crates/axio-quota` is the fifth workspace member, and the reason is
  dependency isolation rather than size. Behind its `app` feature it also
  carries a Tauri desktop surface — tray icon, HTML flyout and window in one
  process, with a React and TypeScript frontend built by Vite — which pulls 189
  packages and a webview runtime. None of that may reach a default `cargo
  install axio`: the feature is off, and `cargo tree -p axio -e normal` names
  no tauri, wry or webview2. The crate is a leaf, depending on no other crate
  in the workspace, and it reads *other tools'* credentials rather than axio's,
  which is a different trust boundary and belongs behind a different name.

  Provider protocol knowledge was derived by reading CodexBar (MIT). No CodexBar
  source is included; see `NOTICE`.
- **A website, at `apps/site`.** Next.js 16, deployed to axio.sh by the Umbra
  control plane as the `axio-site` stack. Built in the Umbra design language
  with amber as axio's product accent — that system already gives each product
  its own hue inside a shared dark shell. It is the first thing in this
  repository that is not Rust; it is not a Cargo workspace member and holds no
  `.rs` files, so `scripts/limits.sh` does not see it, while
  `scripts/firewall.sh` covers it like anything else tracked.
- **Install scripts**, served from the site and living at
  `apps/site/scripts/`:

  ```sh
  curl -fsSL https://axio.sh/install | sh      # macOS, Linux, WSL
  irm https://axio.sh/install.ps1 | iex        # Windows
  ```

  Both build from source with cargo, because nothing is tagged: there is no
  binary to download and no checksum to verify. They check for a toolchain and
  refuse if it is missing or older than 1.88, install into `CARGO_HOME` as the
  invoking user, use no `sudo`, and change no shell profile. `AXIO_INSTALL_REF`
  builds a branch, tag or commit instead of the default. The site serves them as
  `text/plain` so a browser shows the source rather than downloading it.

  Two details in them are load-bearing. The version check compares major and
  minor numerically, because a string compare ranks `1.100` below `1.88` and
  would start rejecting every toolchain the day Rust reaches 1.100. And success
  is reported from the binary just installed rather than from whatever `axio`
  resolves to, because an older copy earlier on `PATH` would otherwise let the
  script confirm a build that never took effect.

- **The site generates its own icons and social card.** It had neither: the tab
  showed the browser's generic globe, `/favicon.ico` was a 404, and a pasted link
  unfurled as a bare title with no image — which for a pre-release whose only
  distribution is someone posting the URL was the whole first impression. A
  32×32 icon, a 180×180 home-screen icon and a 1200×630 card are now rendered at
  build time from the header wordmark and the hero headline, in real Geist, with
  no binary asset committed and no dependency added. `robots.txt` disallows the
  two install routes, which stay reachable for a shell but should not become a
  search result whose entire content is a script, and `sitemap.xml` exists so
  `robots.txt` points at something real. See `apps/site/README.md` for the three
  things in `brand.ts` that are easy to get wrong.
- **A copy button on each install command, and a skip link.** The page's primary
  call to action had been three commands you selected by hand. The button lives
  in the terminal bar rather than over the command, and renders only where the
  Clipboard API exists, so it is never a control that silently does nothing.

### Fixed

- **A fresh install can reach `/login`.** The interactive surface used to build
  its provider before drawing the first frame, so an empty `~/.axio` failed for
  the missing credential and exited before the command that stores one could
  be typed. It now starts with an explicit unavailable-provider state; login
  refreshes the provider picker, and bare `/model` replaces that state in the
  same session. One-shot runs still fail closed.
- **Shell scripts stay LF on Windows checkouts.** Every documented Bash gate
  and the POSIX installer arrived as CRLF under the common `core.autocrlf`
  setting and failed at `set -o pipefail`. `.gitattributes` now makes their
  line ending part of the repository contract.
- **Relocating `AXIO_HOME` no longer reclassifies `~/.axio/config.toml`.** Both
  the active user configuration and the canonical default location are
  excluded from the project-config walk.
- **The site dependency graph is clear of the reported PostCSS and Sharp
  advisories.** Overrides keep Next.js 16.2.12 on patched transitive releases
  until its own dependency ranges catch up.
- **The README said three providers over two implementations.** There are four
  over three — `PROVIDERS` in `crates/axio-core/src/auth.rs` — and has been
  since `openai-codex` landed. The Responses transport was documented elsewhere
  in the same file while that section still described the state before it.

## [0.1.0] — unreleased

First release. A one-shot CLI and an interactive terminal surface, six tools, an
ordered permission engine, sessions on disk, and layered configuration.

### Added

- **The turn loop.** One user turn in, one `TurnEnded` out on every exit path,
  including cancellation and failure. Deterministic three-stage compaction,
  re-derived per step from the transcript so a resumed session compacts at the
  same point as the original.
- **Two providers.** The Anthropic Messages API, and an OpenAI-compatible
  dialect covering Ollama and anything speaking chat-completions. Selected by
  name, not by a plugin system.
- **Six tools** — `read`, `write`, `edit`, `glob`, `grep`, `bash` — behind a
  plan/authorise/execute split, so what was previewed is what runs.
- **An ordered permission engine** keyed on a canonical subject, with a built-in
  deny list for credentials and keys that no user rule and no `--yes` can
  override. A shell command's arguments are tested against it too.
- **Sessions.** Append-only JSONL with a versioned header, `--list`,
  `--resume` by unique id prefix, and `--ephemeral` to record nothing.
- **Layered configuration** — defaults, user file, project file, environment,
  flags — with per-section salvage and `--explain` for provenance. A section that
  will not parse is reset on its own and the file preserved; a section axio does
  not use is ignored quietly, because another tool's config is not a damaged
  one.
- **`--doctor`**, reporting credentials, the effective model and endpoint, the
  provider's real prices, the permission rules in force, and whatever the
  configuration loader complained about.
- **`--json`**, the event stream as one object per line. Unstable, and carries a
  protocol version so a consumer can refuse a stream it does not understand.
- **`axio auth`** — `login`, `status`, `logout`. Credentials are read from stdin
  and stored `0600`; an environment variable always wins.
- **Budgets.** `budget.max_steps` ends a turn that will not stop;
  `budget.max_usd_per_turn` ends one that has spent more than it was given.
  Usage is reported cumulatively per step rather than only at the end, and every
  turn carries its own cost.
- **An exit code for refused work.** A turn that completes with at least one
  action refused exits `5`. The model narrates a success it was never allowed to
  perform, and prose is not something a script can check.
- **Signal handling.** `SIGINT` cancels and flushes a partial answer; a second
  within two seconds gives up. `SIGTERM` and `SIGHUP` cancel and unwind on a
  deadline, taking the whole child process tree with them.
- **An interactive surface.** An inline viewport, so the finished transcript
  lands in the terminal's own scrollback rather than being owned by a
  full-screen application. Approval shows the diff or the raw command before it
  asks. The answer streams in a line at a time with its markdown rendered —
  headings, lists, quotes, rules, code and emphasis — while the unfinished tail
  waits in the viewport, wrapped rather than flattened onto one line.
- **A composer that behaves like a line editor.** Multi-line entry with
  shift-enter or ctrl-j, bracketed paste so a pasted paragraph stays one prompt,
  word-wise movement and deletion, ctrl-u and ctrl-k, and history recall that
  gives back the draft it was interrupted. It grows to three rows and then
  scrolls, keeping what is being typed on screen.
- **Paced drawing.** A fast stream marks the surface dirty far more often than a
  terminal can usefully repaint; frames are coalesced to at most one every 16ms.
- **A status line that says what is happening** — thinking, writing, or the
  subject of the tool being waited on — with a spinner, the turn's elapsed time
  and its token usage, so a quiet model looks different from a hung one.
- **Tables.** Rendered as aligned columns with the alignment the markdown asked
  for, narrowed to fit the terminal rather than overflowing it.
- **A session count in `--doctor`**, under paths, and a note beside any provider
  whose transport has never been run against its real endpoint.
- **The project's own instructions are read.** `AGENTS.md`, or `CLAUDE.md` when
  there is no `AGENTS.md`, from the workspace root — capped, and marked as
  outranking the model's general habits. A codebase that has written down how it
  works should not have to watch every model rediscover it.
- **Argument errors that are enough to fix the call.** Both halves name the tool
  and everything it takes, whether an argument was invented or left out, and a
  call that fails before it has a plan is still labelled with the tool it was —
  so a failure reads as `grep  invalid arguments: …` rather than as a blank.
- **A framed composer.** The prompt sits in a rounded frame carrying the model
  on its top rule and the turn's elapsed time and usage on the right of it, with
  a status bar beneath: what is happening on the left, what to press on the
  right. The frame changes colour for an approval, so the one moment that needs
  an answer does not look like the rest.
- **Tool calls as rows.** A coloured mark for the outcome, the tool's name in
  its own column, what it acted on, and the timing against the right margin.
- **Syntax highlighting in fenced code**, in the terminal's own sixteen colours
  rather than a bundled theme — a theme is chosen against a background, and the
  background belongs to the user. An unknown language renders as plain code.
- **Widths measured in columns.** A CJK character or an emoji occupies two
  columns and counts as one character; counting characters wrote every such line
  a column too wide and lost its end to the clip.
- **An optional sandbox**, Linux only: Landlock, applied before the runtime
  starts and inherited by every command axio spawns. `--sandbox`, or
  `[sandbox] enabled`.

### Dogfooding

axio was asked to add one small feature to its own repository — a line in
`--doctor` reporting how many sessions are on disk — running its interactive
surface against a real model, on a clone, with every approval displayed before
it was answered. Two approvals were asked for and granted: running the test
suite, and one edit to `crates/axio/src/doctor.rs`.

**It produced code that compiled, passed clippy, and was wrong.** The count read
`sessions/` directly, where it found the day directories that session files live
inside rather than the files themselves, so it returned zero and would have gone
on returning zero forever. Nothing about reading it suggested that; running it
did, immediately. It also failed `cargo fmt --check`, which CI would have caught
and no reviewer needed to.

What the surface got right is the part that is hard to test any other way. Four
malformed tool calls — `grep` with invented arguments, `bash` with `cmd` for
`command`, an `edit` whose `old` text was absent, another whose `old` text
matched six times — each came back as a specific, actionable message rather than
a failure, and the model corrected itself each time and carried on. The approval
prompts showed the diff and the command before asking. The turn ended cleanly,
the session recorded, and the transcript is readable.

The feature shipped, rewritten to ask `SessionStore` — which already knew where
session files live — with a regression test that fails against the original
implementation. The honest summary is that axio drove the whole loop competently
and the model's code needed a review it would not have survived without.

A second session ran the identical task from the identical commit, to see
whether the changes the first one prompted had helped. Failed calls were still
rendering against a blank name — the first fix had stopped a long message
truncating the name away, but three separate paths end a call before it has a
subject and only one of them had been patched. That is now fixed at all three,
and it is the clearest argument for doing this at all: two rounds of stub tests
and a pty harness never produced it, and two real sessions produced it twice.

The second session also settled what had *not* worked. Tool descriptions now
name their own parameters, and the invented-argument mistakes of the first
session did not recur — on one run each, which is weak evidence, and is recorded
as weak. The system prompt now asks for the project's own formatter and checks
to be run on what was changed, and **the model ignored it**: unformatted code
again, and the same non-recursive read of a directory that contains directories.
Prompt wording moved neither. What catches both is CI and review.

A third and fourth session settled what the first two could not tell apart:
whether the wrong code was the model's fault or axio's. A stronger cloud model
was given the identical task from the identical commit. Its tool calls were
flawless — no malformed arguments at all, where the first session made four —
and its formatting was clean without being asked. It then made **the same
mistake as both weaker models**, counting session files with a non-recursive
read of a directory that contains directories.

Three models, three identical wrong answers, is not three weak models. The fact
they needed is written down in this repository and axio never showed it to any
of them: it read no project instructions at all. It does now, and the fourth
session — same model, same task, one file visible — reached for
`SessionStore::files()` and got it right. That is the whole finding: the ceiling
was not the model.

**A human did not watch the approvals.** The criterion asks for one, and both
sessions were driven programmatically with each approval captured and answered
automatically. That half remains unverified.

### Known limitations

- **The sandbox is filesystem-only and Linux-only.** It says nothing about the
  network, and on other platforms asking for it is a warning rather than
  confinement. Off by default.
- **The Anthropic path is consistent with the documented wire format but has
  never met the real endpoint.** `scripts/live-check.sh` is what would change
  that, and it has now been run against the chat-completions path — thirteen
  checks, all passing — so the loop, the tools, the deny list and resume are
  proven against something nobody here wrote. Only the Anthropic transport
  remains unproven, and only because no credential exists for it.
- `budget.max_usd_per_turn` cannot fire on a provider that reports no prices.
  `--doctor` says so rather than leaving it looking enforced.

[Unreleased]: https://github.com/umbra-me/axio/commits/main
