# axio

A cross-platform AI coding agent — Rust, with a one-shot CLI and an
interactive terminal interface.

axio runs its own agent loop: it talks to LLM providers directly, executes tool
calls against your workspace, and streams the result back to you.

This repo is a from-scratch rewrite; the previous implementation is archived and
read-only.

The agent has no hosted accounts or product backend. Umbra operates the product
record and axio.sh website centrally through `admin.umbra.me`; there is no
standalone Axio admin site. The website exposes a read-only Product Admin
adapter for current product and site status without reviving hosted accounts or
the retired cloud. The website is a managed
portfolio surface with central health, non-secret configuration, and
first-party page-load analytics. Axio's local sessions, provider credentials,
and transcripts never become hosted account data.

Documentation reconciliation (2026-08-05): the published product pin and
control-plane checkout were synchronized without rebuilding this website. The
local-first product boundary above is unchanged; checkout alignment is not a
claim that a new runtime image was deployed.

## Status

Early, but it works. `scripts/live-check.sh` has been run end to end against a
real model on **both** the chat-completions and the Responses paths, and on
Windows — thirteen checks over seven turns, each time
covering the turn loop, the transport, the one-shot CLI, the read, write and
bash tools, a refusal and its exit code, the built-in deny list keeping a secret
out of both the answer and the transcript, sessions on disk, and resume carrying
its history. It can read, search, edit and run commands in a workspace, and pick
up where it left off.

The Responses transport has met its endpoint too: signed in through the browser
flow, then the same thirteen checks green against it. Its request body took two
live 400s to settle — a field that exists in the dialect and is refused by the
endpoint, and a model catalogue that had to be asked for rather than assumed.

The Anthropic transport has not had that run against it: there has been no
credential to run it with. It is built from the documented wire format and
snapshot-tested against it, which is not the same as having met the endpoint.

It has also been pointed at its own repository and asked to add a feature, four
times across two models — which is where its own remaining bugs have come from
lately. All four are written up in the changelog, the code it got wrong along
with them, including the run that proved the ceiling was axio's context and not
the model's ability.

**There is no release yet.** Nothing is tagged; `cargo install --git` builds
whatever `main` is. The version will be cut when a few days have passed without
real use turning something up, not before.

Writes and shell commands need approval; reads do not. Interactively you are
asked. In a one-shot run there is nobody to ask, so they are refused unless
`--yes` was given — and a turn that completed with something refused exits `5`,
so `&&` sees it.

## Install

No published binary yet — releases are built by tag, and none has been cut. So
every route below compiles the current `main`; there is nothing to download and
nothing to checksum.

```sh
curl -fsSL https://axio.sh/install | sh      # macOS, Linux, WSL
irm https://axio.sh/install.ps1 | iex        # Windows
```

Those scripts live in this repository at `apps/site/scripts/`, and the site
serves them as `text/plain` so a browser shows the source rather than
downloading it. Read one before running it — that applies to every script anyone
asks you to pipe into a shell, including these. They check for a toolchain and
refuse if it is missing or older than 1.88, install into `CARGO_HOME` as the
invoking user, use no `sudo`, and change no shell profile. `AXIO_INSTALL_REF`
builds a branch, tag or commit instead of the default.

Driving cargo yourself does the same thing:

```sh
cargo install --git https://github.com/umbra-me/axio --locked axio
```

Needs a Rust toolchain at 1.88 or newer. The binary lands in `~/.cargo/bin`.
From a clone, `cargo install --path crates/axio --locked` does the same.

`cargo install axio` from crates.io does **not** work: that name belongs to an
unrelated crate.

## Use

```sh
axio auth login                     # stores a credential, 0600, read from stdin
export ANTHROPIC_API_KEY=sk-ant-... # or just use the environment

axio -p "explain this repo"
cat src/lib.rs | axio -p "review this"
axio --doctor                       # what axio can currently see, offline
axio --probe                        # ask the model whether it accepts tools

axio                                # interactive, if stdin is a terminal
                                    #   `/` opens the command menu
                                    #   `/status` what this session is set to do
                                    #   `/model NAME` change model mid-session
                                    #   `/login` store a key, or sign in via browser
axio --list                         # recent sessions
axio --resume 01K3F               # continue one; a unique prefix is enough
axio --explain model.effort         # where a setting came from
axio --ephemeral -p "..."           # record nothing
axio --model NAME -p "..."          # override the model for this run
axio --sandbox -p "..."             # confine commands to the workspace (Linux)
```

`--doctor` answers from configuration alone: no credential, no socket, safe
anywhere. `--probe` is the opposite by design — it sends two short requests to
the configured model, one of them carrying a tool, and reports whether the tool
was accepted. A model can serve chat perfectly and reject every request that
offers it a tool; nothing in the configuration is wrong when that happens, so
only asking the model finds it.

Configuration is layered — defaults, then `~/.axio/config.toml`, then the
nearest project `.axio/config.toml`, then `AXIO_*` variables, then flags.
`~/.axio` is the same path on Windows, WSL, Linux and macOS rather than each
platform's own configuration directory: one path to document, to sync between
machines and to name in a bug report. `AXIO_HOME` moves it.

```toml
[model]
effort = "xhigh"

[budget]
max_usd_per_turn = 2.0
max_steps = 50

[permissions]
deny = ["bash:curl"]

[sandbox]          # Linux only, off by default
enabled = true
read = ["/home/me/.pyenv"]
write = ["/home/me/.cache/go-build"]

[worktree]         # supervised sessions; on by default
enabled = false    # user config only — see below
branch_prefix = "axio/"
```

A project's own `.axio/config.toml` may only add restrictions, never remove
them. That covers `[permissions] allow`, and it covers `[worktree] enabled =
false` — which does not look like a permission, and is the point. Supervised
sessions each get their own git worktree and branch, so an agent edits an
isolated checkout rather than the one you are using; a repository that could
switch that off would be deciding, for anyone who cloned it, that its agents
may write to your working tree. Turning it off is yours to do, in your own
config or for one session at a time. Sections axio does not use are ignored — it will not complain about, or
touch, a file that belongs to something else.

If the workspace root has an `AGENTS.md` — or a `CLAUDE.md`, when there is no
`AGENTS.md` — axio reads it and tells the model those instructions describe this
codebase specifically and outrank its general habits. It is capped, since it is
prepended to every request in the session. This is worth writing: three models
asked to touch this repository made the same wrong assumption until the file that
corrects it was actually shown to them.

## The interactive interface

Run `axio` with nothing piped in and no `-p`, and you get a composer instead of
a single turn.

It is **inline**, not a full-screen application: the finished transcript is
printed into the terminal's own scrollback, so it survives the process, scrolls
with the scrollbar and copies with the mouse. Only the live part — a status
line, the composer, the question being asked — is redrawn.

The answer arrives a line at a time with its markdown rendered: headings, lists,
quotes, code, tables and emphasis become styles and columns rather than
asterisks and pipes, and the sentence still being written waits dim in the
viewport until it is finished. Fenced code is highlighted in your terminal's own
colours rather than a bundled theme, so it matches everything around it.

```
  ⏺ read    src/parse.rs                              3ms
  ⏺ edit    src/parse.rs  +12 −4                     18ms

  Done — the lexer now owns the span table.

╭─ claude-opus-5 ───────────── 4s · 1.2k in / 340 out ─╮
│ › explain the change                                 │
╰──────────────────────────────────────────────────────╯
  · thinking                        ctrl-c to interrupt
```

The composer is a line editor. A pasted paragraph stays one prompt rather than
submitting its first line.

| Key | Does |
| --- | --- |
| `enter` | send |
| `shift-enter`, `ctrl-j` | another line |
| `up`, `down` | move a line, recall history when there is only one, or move the slash menu when one is open |
| `ctrl-w`, `ctrl-u`, `ctrl-k` | delete a word, to the start, to the end |
| `ctrl-left`, `ctrl-right` | move a word |
| `esc`, `ctrl-c` | interrupt a turn; twice at an empty prompt to leave |
| `ctrl-d`, `/exit` | leave |

### Slash commands

Typing `/` opens a menu of things the surface answers itself. None of them
reach the model, spend a token or touch the transcript, which is why the menu
can open on a keystroke.

```
› /help    list these commands
  /status  what this session is configured to do
  /model   pick a provider and model, or `/model NAME`
```

`↑` `↓` move, `tab` completes the name into the line, `enter` runs the
highlighted one, `esc` dismisses. The menu closes at the first space — from
there the words are an argument. A name that matches nothing is refused rather
than sent: a mistyped command should not become a prompt and spend a turn
answering a typo.

| Command | Does |
| --- | --- |
| `/help` | list the commands |
| `/status` | model, provider, endpoint, credential source, permissions, workspace |
| `/model` | pick from what the provider serves; `/model NAME` sets one directly |
| `/login` | store a credential, or sign in to one that uses a browser |
| `/clear` | discard what is in the composer |
| `/quit` | leave |

Bare `/model` asks which provider first, then what to run on it:

```
›  1. ollama          ✓
   2. anthropic         not configured
   3. openai-codex
```

Both stages take `↑` `↓`, `1`–`9`, `enter` and `esc`. Provider and model are
chosen together rather than by two commands, because changing the provider on
its own would leave the session holding a model its new endpoint has never
heard of. A provider with no credential stays on the list and cannot be
chosen — removing it would read as one axio cannot reach, when the answer is
`/login` away.

The model list is fetched from every provider, never compiled in: a name missing
from a built-in catalogue looks exactly like a name the provider refuses. The
one in use is ticked and the highlight opens on it, so the reflex Enter changes
nothing.

A switch that works becomes the default. It is written **after** the next turn
comes back, never when it is chosen — the name is not checked until then, so
saving at the moment of choosing would make a typo the default and every later
session would start broken. Only an outcome that could not have happened
without the endpoint accepting the request counts, so a refusal saves and a
transport failure does not. The file is line-edited rather than rewritten, so
comments and sections axio does not use survive.

`/model NAME` changes the name in the request body and nothing else, so it
reaches only models the configured provider already serves. Moving away from the model
that minted the transcript drops that model's reasoning from every later
request; it says so when it happens. The name is not checked until the next
request, so a typo surfaces then rather than at the prompt.

`openai-codex` is signed in to rather than pasted into: choosing it opens a
browser, catches the redirect on the loopback and stores the token pair, which
axio then renews before it expires. Requests go to the subscription endpoint,
which speaks a third dialect. The client id used is Codex's own — a public
identifier, but presenting it is outside what a third-party client is
authorised to do, and the module holding it says so.

For the providers that take a key, `/login` runs in the viewport rather than
the shell. What is typed is never
drawn — only how many characters there are — and never reaches scrollback,
which outlives the process. A pasted key loses its trailing newline, because a
credential with one in it is rejected as simply wrong. The session keeps the
credential it started with until bare `/model` replaces the provider. The newly
stored credential appears in that picker immediately.

A fresh install can open the interface without a credential. `/login` remains
available there; after it stores or obtains one, bare `/model` refreshes the
provider list and moves the same session onto a provider and model it can use.
No restart is required. `/model NAME` still changes only the model on the
provider already in use.

When an action needs approval the diff or the command lands in scrollback and
the viewport asks:

```
  approve  edit:notes.md
  this will write files
  @@ -1,3 +1,4 @@
   # Notes
   - first
  +- second

  allow? y once  a this session  n no
```

A shell command is shown as the string the shell actually receives, never a
word-split of it: the split reads as a simpler command than the one that runs.

## Providers

Four names over three wire dialects, not a plugin system — the list is
`PROVIDERS` in `crates/axio-core/src/auth.rs`. `anthropic` speaks the Messages
API; `ollama` and `openai-compatible` share the chat-completions one, the second
for any other host speaking that dialect, pointed at it with `model.base_url` or
`AXIO_BASE_URL`; `openai-codex` speaks the Responses API and is signed in to
through the browser rather than pasted into, as described above:

```sh
axio auth login                             # the default provider
axio auth login --provider ollama < key.txt
axio auth status
axio auth logout --provider ollama
```

Credentials are stored at `$AXIO_HOME/auth.json`, `0600` on unix, and an
environment variable (`ANTHROPIC_API_KEY`, `OLLAMA_API_KEY`) always takes
precedence. Nothing ever prints the credential back.

```sh
AXIO_PROVIDER=ollama AXIO_MODEL=gpt-oss:120b axio -p "..."
```

The second exists mostly to keep the first honest: implementing a differently
shaped API is how you find out whether an abstraction is one.

Actions that change something ask first. `--yes` answers every one of those
prompts with yes — unattended and unsandboxed, though the built-in deny list
still refuses what it refuses — so read [`SECURITY.md`](SECURITY.md) before
reaching for it.

`--json` emits the event stream as one object per line. It is unstable and
carries a `protocol` version so a consumer can refuse a stream it does not
understand.

## Quota

`axio quota` reports how much of each provider's limit is left and when it
resets, across ten providers: Codex, Claude, OpenRouter, z.ai, DeepSeek, xAI,
Grok, Cursor, Ollama and opencode. **It never reads axio's own stored
credentials** — a quota probe and an agent turn are different trust boundaries.

They arrive by three routes, because the vendors offer three. Six are read from
a credential another tool already wrote — the files `codex`, `claude`, `grok`
and Cursor keep for themselves — so those need no configuration at all. Three
are API-key providers with one endpoint each. The rest have no API for it, and
are reached with a browser session you grant from the desktop app.

```
> axio quota
Codex (pro)
  Weekly                        22% used  resets in 5d
Claude (max)
  5h                             8% used  resets in 2h
  Weekly                         2% used  resets in 6d
  Weekly (Fable)                 0% used
```

`--json` for one object per provider, `--diagnose` to see where each probe looks
and whether the credential is there.

There is also a desktop app — tray icon, a flyout panel against it, and a window
with detail, history, cost and settings. It is behind a feature so a default
install compiles none of it:

```sh
npm --prefix crates/axio-quota/ui run build
cargo build --release -p axio-quota --features app
```

For the providers that expose usage only to a signed-in browser, the app signs
you in rather than asking you to paste a cookie header. A button opens the
**provider's own page** in a window, you sign in to that vendor as you would in
a browser, and the session lands in that webview. The credential goes only to
the vendor — no relay, no extension — and nothing on your machine is decrypted:
reading the browser's own cookie jar would mean going after a key the OS holds
for another application, which is a lot of machinery to take a credential you
can simply grant. The window is deliberately decorated, so a vendor's sign-in
form never appears inside chrome that looks like ours, and it cannot reach
axio's command surface.

Provider knowledge was derived by reading
[CodexBar](https://github.com/steipete/codexbar) (MIT); no source is included.
See [`crates/axio-quota/README.md`](crates/axio-quota/README.md), `NOTICE`, and
[`crates/axio-quota/PORTING.md`](crates/axio-quota/PORTING.md) for what the port
carried over and what it left behind.

## Cost

`axio cost` reports what the coding agents on this machine have spent, read from
the session transcripts they already write. No network, no credentials, nothing
to configure.

```sh
axio cost                                   # everything, grouped by model
axio cost --by client|session|day|workspace|week|month|hour
axio cost --calendar                        # the year as a shaded calendar
axio cost --wide                            # cache share, $/M, share of total
axio cost --json                            # for scripting
axio cost --cached                          # read the saved scan, skip rescanning
axio cost --diagnose                        # what each parser found and skipped
curl -fsSL https://models.dev/api.json -o p.json && axio cost --import-prices p.json
```

Twenty-three agents are covered. Fifteen in a default build; the other eight
keep their sessions in SQLite and sit behind the non-default `sqlite` feature,
because reaching them means compiling SQLite's C source and `cargo install axio`
must not need a C toolchain. An agent whose directory is absent reports *not
installed*, which is a different answer from *recorded nothing*.

Two rules the numbers rest on. **A model with no known rate is reported
unpriced, never as zero**, and no total prints without the share of tokens it
accounts for. And every total is broken down by provider and by harness —
different questions, which genuinely diverge when a CLI is pointed at a proxy.

## Shape

Seven crates and two binaries:

| Crate           | Contains                                                        |
| --------------- | --------------------------------------------------------------- |
| `axio-core`     | Conversation state, turn loop, `Tool` and `Provider` traits      |
| `axio-provider` | Provider HTTP client, streaming, request/response types          |
| `axio-tools`    | `read`, `write`, `edit`, `bash`, `glob`, `grep`                  |
| `axio-quota`    | Provider limit probes, local history, and the desktop app behind `app` |
| `axio-cost`     | What the agents on this machine have spent, read from their own transcripts |
| `axio-supervisor` | Many sessions at once, across many repositories — a worktree and branch each, one queue of approvals |
| `axio`          | The binary — one-shot CLI when piped or given `-p`, interactive on a TTY |

`axio-quota` and `axio-cost` are leaves: they depend on no other crate here, and
only `axio` depends on them. Both read files that belong to *other* tools rather
than to axio, which is a different trust boundary and why each sits behind its
own name.

The core emits a stream of events and knows nothing about rendering, so each
surface is a consumer of the same channel. `--json` is a second renderer, never
a second loop.

`apps/site` is the axio.sh website and the only thing here that is not Rust: a
Next.js app, plus the two install scripts the site serves. It is not a Cargo
workspace member and holds no `.rs` files, so `scripts/limits.sh` — which counts
members and Rust lines under `crates/` — does not see it. `scripts/firewall.sh`
greps the whole tracked tree, so it applies there like anywhere else.

See [`docs/architecture.md`](docs/architecture.md) for the invariants,
[`docs/gotchas.md`](docs/gotchas.md) for the traps, and
[`docs/roadmap.md`](docs/roadmap.md) for what is coming and what is deliberately
not.

## Safety

axio executes code written by a language model against your working directory.
By default there is no sandbox: confinement is the workspace root, the approval
prompt and process-group containment.

On Linux, `--sandbox` adds one — Landlock, the kernel's own, inherited by every
command axio spawns. The workspace is writable, the system is readable, and
`~/.ssh` is not there at all. It says nothing about the network, and it is a
second wall behind the permission engine rather than a replacement for it.

Read [`SECURITY.md`](SECURITY.md) before running it anywhere that matters.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
