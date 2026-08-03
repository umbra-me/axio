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

  Twenty-three agents are covered. Four have hand-written parsers — Claude Code,
  Codex, Grok and opencode — because each carries knowledge a generic walk
  cannot infer: which of two token figures is cumulative, which repeated events
  must be suppressed, which vendor reports its own cost, which convention
  counts reasoning twice. The other twelve are rows in `sources::catalog`,
  driven by one table-driven parser, because these formats log the same event
  and differ only in where they put it.

  Eight of them keep sessions in SQLite rather than files and sit behind the
  non-default `sqlite` feature, because reaching them means compiling SQLite's C
  source and `cargo install axio` must not need a C toolchain. Seven reuse the
  same walker: nearly every one keeps a JSON blob in a column, so the database
  part is only getting to the JSON. A default build therefore covers fifteen,
  and the Settings tab lists the absent ones greyed rather than omitting them —
  *not installed* is an answer, and a list that hides them cannot distinguish it
  from *found nothing*.

  opencode is the fourth hand-written parser and the one that could not be a
  table row: its own total proves that `total = input + output + reasoning +
  cache.read`, so **reasoning is added to output there where Codex contains it
  within output**. `TokenBreakdown` holds reasoning as a subset, so the parser
  folds it in and keeps a copy for reporting. Mapped straight across it would
  vanish from every total — one observed message carries 9,459 reasoning tokens
  against 171 of output, so that row would have read 98% short.

  An agent whose directory is absent reports *not installed*, which is a
  different answer from *recorded nothing* and is what `--diagnose` prints.

  The scan runs across threads — agents in parallel, and each agent's files split
  across workers again, because one agent usually holds most of the files and
  per-agent parallelism alone leaves a single thread doing nearly all the work.
  Measured on this machine: 6.02s to 1.42s, with byte-identical totals.
  `AXIO_COST_THREADS` pins it to one thread for debugging.

  The desktop app's Cost tab shows the same table, grouped by model, agent, day,
  workspace or session. The scan is cached because reading every transcript takes
  tens of seconds; regrouping is instant, and a `rescan` button drops the cache
  when a figure needs to be current. The view distinguishes an unpriced row from
  a cheap one exactly as the CLI does.

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

  Every model observed in the local transcripts is priced, so the totals carry
  no coverage caveat. Three vendors needed their cache-read rate listed rather
  than derived: the tenth-of-input that Anthropic and OpenAI share is nowhere
  near right for DeepSeek (2% of input), Z.ai (19%) or xAI (25%).

  Every total is broken down by **provider** and by **harness**, whichever the
  table is grouped by. The two are different questions — who is going to invoice
  me, and which tool spent it — and they genuinely diverge: $449 of the Claude
  Code usage on this machine was billed by OpenAI, because the CLI was pointed at
  a proxy. The provider is derived from the model, never from the directory the
  log sits in.

  `--import-prices` takes a models.dev-shaped feed so a model the bundled table
  has never heard of can still be costed. It is a file rather than a fetch:
  `axio-provider` is the only crate here that links HTTP, and one convenience is
  not worth spending that boundary on. The bundle outranks the feed for models it
  knows, because a bundled row carries tier and promotional structure a flat feed
  row cannot express and would silently drop.

  **A model with no known rate is reported unpriced, never as zero**, and no total
  is printed without the share of tokens it accounts for. That rule has teeth: an
  early build summed the one Codex message in 77,525 whose model it knew and
  printed `$0.30` next to the word *Codex*. `Totals` now hands back a `Cost` that
  cannot be formatted without confronting its own coverage.

- **`axio quota`, and a desktop app behind it.** How much of each provider's
  limit is left and when it resets, across ten providers — Codex, Claude,
  OpenRouter, z.ai, DeepSeek, xAI, Grok, Cursor, Ollama and opencode. `--json`
  emits one object per provider, `--diagnose` prints where each probe looks and
  whether the credential is there.

  They arrive by three different routes, because the vendors offer three. Six
  are read from a credential another tool already wrote — the files `codex`,
  `claude`, `grok` and Cursor keep for themselves — so those need no
  configuration at all. Three are API-key providers with one endpoint each.
  The rest have no API for it, and are reached with the browser session
  described below. Nothing here reads axio's own stored credentials; a quota
  probe and an agent turn are different trust boundaries.

  Each vendor needed one thing decided rather than copied. z.ai reports windows
  in its own vocabulary of unit codes, and stamps them in milliseconds — read as
  seconds that lands in 1970 and reports the window as permanently resetting; an
  unrecognised unit keeps its percentage under a vague label rather than being
  dropped, because the one dropped could be the one about to run out. DeepSeek
  returns a balance per currency as strings, and USD is preferred rather than
  summed, since adding CNY to USD produces a number that is not money in any
  currency. xAI posts an inverted ledger in string cents, so $10 arrives as
  `-1000`. Cursor's percentages are percentages even below one — `0.36` means
  0.36%, and reading it as a fraction would turn a reading the dashboard rounds
  to zero into a third of the plan. Ollama is scraped, because the Cloud Usage
  bars are not on the API at all, and an unrecognised layout is an error rather
  than a guess.

  **A response with no parseable figure is an error, never a zero.** "Nothing
  left" and "we could not tell" must not render the same.

- **Signing in to a cookie provider, in a window.** Pasting a `cookie:` header
  asks someone to know which of thirty cookies is the session, and both obvious
  ways to copy one produce something every server refuses: a bare value with no
  name, or the header with its own name still attached. The pastes on this
  machine were all the first kind.

  So the app signs in instead. A button opens the provider's own page in a
  window, the user signs in to that vendor as they would in a browser, and the
  cookies land in that webview where they are read back. The whole jar for the
  origin is kept, because a dashboard request carries all of them and some of
  these endpoints check a CSRF or region cookie alongside the session.

  Two properties this shape has that the alternatives do not. The credential
  goes only to the vendor — no relay, no extension, and the password is typed
  into the provider's own page over TLS. And **nothing is decrypted**: reading
  the browser's own jar would mean DPAPI and AES-GCM against a key the OS holds
  for another application, which is a lot of machinery to take a credential the
  user can simply grant.

  The window is deliberately decorated, so somebody else's sign-in form does not
  appear inside chrome that looks like ours — that is the shape of a phishing
  screen. It is absent from `capabilities/default.json`, which is the
  load-bearing part: a remote page must not reach axio's command surface.

  Capture is polled rather than pushed, because a site can set its session on a
  redirect, on a background request, or after a second factor, and none of those
  is a navigation event. **A cookie's name is not proof it is a session**: the
  candidate is used once against the provider's own endpoint, and only a
  credential that actually authenticates is accepted. Only Unauthorized means
  keep waiting — a missing workspace or a network blip both mean the session
  *was* recognised, and treating either as "not yet" would leave the window open
  forever on an account that is signed in and has nothing to report.

- **Cursor's session is imported from Cursor's own store.** Anyone with Cursor
  installed and signed in already holds the credential its dashboard uses, so
  there is nothing to sign in to and nothing to paste. Worth being precise about
  what this reads, because the neighbouring idea is much worse: it opens
  Cursor's own state database, not a browser's cookie store. No decryption, no
  DPAPI, no key belonging to another application — the token is sitting there in
  plain text because Cursor put it there for itself. The database is copied
  before it is read, since opening a live SQLite file either fails or recovers a
  journal into a file another program is still using. A pasted header still
  wins: that is very likely a second account, and silently preferring the local
  one would report the wrong account's usage.

- **Refresh intervals are computed, not chosen.** The five-minute constant this
  replaces documented its own problem — far more often than a weekly window
  needs, far too slow to watch a session window drain under load — and the cost
  of being wrong is asymmetric in both directions: too often and the usage
  endpoint rate-limits the tray that was reading it, too rarely and a window
  empties, resets and refills without ever being shown.

  Adaptive paces at a twentieth of the time remaining on the nearest window,
  which gives about twenty readings before it turns over, clamped to half an
  hour; inside ten minutes of a reset or above 90% used it drops to a
  one-minute floor. A failed probe *shortens* the interval, because treating an
  error as 0% used would slow the loop down at exactly the moment a retry is
  wanted. The policy takes the current time as an argument rather than reading
  it, so every branch is a test rather than something you find out by waiting.

- **The year as a calendar, and a Stats tab.** The tables answer what was spent;
  a year of days answers when the work actually happened, which is a shape
  rather than a number and no grouping of a table reaches it. `axio cost
  --calendar` and the app's Stats tab run over the same `summarise()`, so a day
  is shaded identically in each.

  Levels are quartiles of the active days, not a ramp scaled against the busiest
  one. Both scalings were tried against real data: with a peak thirty times the
  median, linear puts nearly every day in the lowest bucket and logarithmic puts
  nearly every day in the highest — 50M against 1.8B is 83% of the way up a log
  scale. Quartiles compare a day to a typical day instead, which is what someone
  reading their own calendar means.

  Stats also carries daily spend, tokens by hour and weekday, the token mix, and
  the top providers, harnesses and workspaces as ranked bars. The token mix
  earns its place: cache reads are most of a coding agent's volume and a tenth
  of its price, so a total that does not separate them points at the wrong
  culprit. Reasoning is shown as a share of output rather than beside it,
  because it is billed as output and listing the two side by side reads as
  double counting. Hours are UTC and labelled UTC rather than converted — a
  histogram that shifts when you travel is worse than one honest about its
  clock.

- **Derived columns, and coarser groupings.** Rows carry the share of their
  tokens that were cache reads, the blended dollars per million priced tokens,
  and their share of the total. Groupings gain week, month and hour — hour
  pooled across every day, which answers when the work happens rather than when
  it happened.

  The cache column began as a multiple of fresh input and was wrong to: measured
  against real data it put one model at 31x and another at 121,077x, because the
  vendors do not mean the same thing by "input" — OpenAI counts the whole
  prompt, Anthropic counts only what missed cache. Two numbers in one column
  that cannot be compared to each other is worse than no column. As a share of
  the row's tokens it means the same thing whoever reported it, and reads 90-99%
  across every vendor. Dollars per million divides by the tokens that were
  *priced*, since dividing a partial cost by a whole volume understates the rate
  by exactly the share that had no price.

  The wide table is behind `--wide` in the CLI, where eighty columns is still a
  constraint, and always on in the window, which has the room.

- **The scan is saved, and reads back in a fraction of the time.** Nothing about
  it changes between runs — the transcripts are append-only and mostly untouched
  — so paying half a minute to rediscover it on every launch was the whole cost
  of not writing it down. JSONL, one record per line. Reading it back is 0.67s
  against 2.3s to scan, and the totals round-trip exactly. `axio cost --cached`
  reads the same file the app does, so the CLI and the window share one scan and
  either can produce it. It lives under `LOCALAPPDATA` rather than the roaming
  profile: it is tens of megabytes of rebuildable data describing files on this
  machine only.

  Flattening the scan onto one pool came with it. It had spawned a thread per
  agent and split each agent's files across workers again, so the thread count
  was the product of the two — twenty-two agents against sixteen workers
  reserves stacks for over a hundred threads on a machine that runs eight — and
  it balanced badly, because one agent here holds 449 files and another holds 2,
  and the second got a whole worker while the first queued. Gathering the file
  lists first and splitting once took the speedup against
  `AXIO_COST_THREADS=1` from 2.2x to 3x, with totals unchanged run for run.

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

- **The app no longer freezes on the first Cost or Stats view**, and the cause
  was one keyword: a Tauri command declared `fn` rather than `async fn` runs on
  the main thread, which is also the thread that paints and handles input. The
  cache scanned inline on first use, so the window was dead for the whole scan —
  and it held the mutex throughout, so the second view queued behind the first
  and froze again. The scan now runs on a worker started at launch, in two
  phases: the saved scan is published within milliseconds so a tab has real
  figures before anyone reaches it, then the live scan replaces it and emits
  `cost://updated`. Refreshing returns as soon as the worker starts rather than
  blocking until it finishes, with the previous figures left on screen flagged
  as scanning — a stale number beats an empty table.

- **Opening the sign-in window no longer deadlocks the app.** Reading cookies
  posts a message to the event loop and blocks on the reply, so doing it from a
  synchronous command deadlocks — Tauri documents this on `Webview::cookies` and
  the first version ignored it. The capture is async now and reads on a thread
  of its own, a missing window is checked before anything blocking rather than
  after, and opening the window was moved off the main thread too: building a
  webview waits on the event loop, and a synchronous command *is* the event
  loop. The poll also waits for each answer before scheduling the next, rather
  than stacking a fresh check on top of a page still loading.

- **The "rate limited by provider" was ours.** Saving settings triggers a
  refresh, so does capturing a sign-in, so does the schedule, and so does the
  button — under a run of edits those pile onto an endpoint that rate limits,
  and the view reported the vendor's 429 as though the vendor were at fault.
  Twenty seconds between probes collapses a burst into one. The Refresh button
  bypasses it, because someone pressing it is asking a question the throttle
  would otherwise decline to answer silently.

- **The History tab stopped losing and duplicating series.** A series was keyed
  by joining the provider and the window label into one string and splitting it
  back on a space, which silently truncated every label containing one: "Weekly
  (Fable)" became a second "Weekly" and drew a duplicate chart against the wrong
  data, while "GPT-5.3-Codex-Spark Weekly" matched no reading at all and
  vanished from the tab entirely. It groups on the pair now, keyed as JSON
  rather than on a separator character.

- **The History charts are styled, and the file is no longer binary.** The tab
  had no CSS at all, so each chart fell back to its viewBox ratio and stood
  130px tall; they are 46px sparklines now, framed, dated at both ends. The line
  was also amber, which breaks the sheet's first rule — amber is the product
  accent and draws an edge or a mark, never a fill — so a line stating
  consumption now takes the ok/warn/crit ramp and agrees with the rail on the
  Providers tab for the same number. The same file carried two NUL bytes, which
  is why git had been treating it as binary and refusing to diff it; no other
  tracked source file has any. Also in it: the password field rendered as a
  white box on a black page, the cost table's total row fell below the fold and
  is now pinned, and the Stats cards ellipsed `18,374,235,696` to
  `18,374,235,6...` — which reads as a smaller number rather than a truncated
  one, and is now `18.37B` with the digits on hover.

- **The frameless window can be dragged again.** The app had no `capabilities/`
  directory at all, so Tauri v2 denied `core:window:allow-start-dragging` and
  the window simply did not move — no error, no warning. The minimise and close
  buttons kept working because Rust-side calls bypass the permission layer
  entirely, which made the two look like unrelated problems. The drag rule is
  inverted while there: everything drags except what responds to a click, so the
  next element somebody adds is draggable without anyone remembering to say so.

- **A refused cookie paste now says why it was refused.** The stored headers for
  all three cookie providers contained no `=` anywhere — they were bare cookie
  *values*, which is what the browser's cookie table gives you when you click a
  row, and much easier to find than the request headers. A bare value is now
  named rather than rejected, taken as the value of the provider's own session
  cookie; a leading `Cookie:` from the other copy route is stripped, since sent
  verbatim it produces `Cookie: Cookie: a=1`. Errors name the status and, on a
  redirect, where it pointed — minus the query string, which on a sign-in
  redirect carries the original URL and sometimes a token. `--diagnose` reports
  how many cookies a paste holds, whether any is a session cookie, and which
  names would count: that is the question someone actually has when a paste is
  refused.

- **opencode's usage fields are no longer guessed at one spelling.** The session
  and workspace lookup were both working, and the parser still reported "no
  rolling usage found — the session may have expired", which is a confident
  diagnosis of the wrong thing that sends anyone reading it to sign in again for
  no reason. The response is not a documented API but whatever the site's own
  server function returns, and it names things several ways. It now tries the
  full list of spellings for both the percentage and the reset countdown, and
  when it still finds nothing it reports the identifier-like keys the response
  actually contained — **names only, never values**, because these payloads can
  carry an account's email. A parser that fails silently on an undocumented
  shape is one nobody can fix.

- **The opencode workspace is found rather than asked for.** It had needed a
  workspace id typed into Settings before it would report anything, and a
  session that had just been granted still showed "no workspace" — which reads
  as a failed sign-in. The site already knows which workspaces a session can
  see, so it is asked; an explicitly configured id still wins and skips the
  lookup. The cookie is checked before the workspace now, because the other
  order reported every signed-out probe as a workspace problem and sent the fix
  in the wrong direction. Half-configured providers also stopped vanishing from
  the view: a provider with some credential keeps its error, where before the
  work was done and there was neither acknowledgement nor instruction.

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
