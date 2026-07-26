# axio

A cross-platform AI coding agent — Rust, with a one-shot CLI and an
interactive terminal interface.

axio runs its own agent loop: it talks to LLM providers directly, executes tool
calls against your workspace, and streams the result back to you.

This repo is a from-scratch rewrite; the previous implementation is archived and
read-only.

## Status

Early, but it works, and `scripts/live-check.sh` proves it against a real model
rather than a stub: the turn loop, the transport, the one-shot CLI, all six
tools, the deny list, sessions on disk, and layered configuration. It can read,
search, edit and run commands in a workspace, and pick up where it left off.

Writes and shell commands ask before they happen; reads do not.

One caveat worth stating plainly: while the OpenAI-compatible provider is
exercised against a live endpoint on every check, the Anthropic path has only
ever been run against the documented wire format, never the real API.

## Install

No published binary yet — releases are built by tag, and none has been cut.
Until then, and for the current `main`:

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
axio --doctor                       # what axio can currently see

axio                                # interactive, if stdin is a terminal
axio --list                         # recent sessions
axio --resume 01K3F               # continue one; a unique prefix is enough
axio --explain model.effort         # where a setting came from
axio --ephemeral -p "..."           # record nothing
```

Configuration is layered — defaults, then `~/.config/axio/config.toml`, then the
nearest `.axio/config.toml`, then `AXIO_*` variables, then flags:

```toml
[model]
effort = "xhigh"

[budget]
max_usd_per_turn = 2.0
max_steps = 50

[permissions]
deny = ["bash:curl"]
```

A project's own `.axio/config.toml` may only add restrictions, never remove
them.

## The interactive interface

Run `axio` with nothing piped in and no `-p`, and you get a composer instead of
a single turn.

It is **inline**, not a full-screen application: the finished transcript is
printed into the terminal's own scrollback, so it survives the process, scrolls
with the scrollbar and copies with the mouse. Only the live part — a status
line, the composer, the question being asked — is redrawn.

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

`enter` sends · `ctrl-c` interrupts a running turn, or exits at an empty prompt
· `ctrl-d` leaves · `up`/`down` walk what you have already asked.

A shell command is shown as the string the shell actually receives, never a
word-split of it: the split reads as a simpler command than the one that runs.

## Providers

Two, selected by name — not a plugin system:

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

Actions that change something ask first. `--yes` approves everything without
asking — unattended and unsandboxed, so read [`SECURITY.md`](SECURITY.md) before
reaching for it.

`--json` emits the event stream as one object per line. It is unstable and
carries a `protocol` version so a consumer can refuse a stream it does not
understand.

## Shape

Four crates, and one binary serving both surfaces:

| Crate           | Contains                                                        |
| --------------- | --------------------------------------------------------------- |
| `axio-core`     | Conversation state, turn loop, `Tool` and `Provider` traits      |
| `axio-provider` | Provider HTTP client, streaming, request/response types          |
| `axio-tools`    | `read`, `write`, `edit`, `bash`, `glob`, `grep`                  |
| `axio`          | The binary — one-shot CLI when piped or given `-p`, interactive on a TTY |

The core emits a stream of events and knows nothing about rendering, so each
surface is a consumer of the same channel. `--json` is a second renderer, never
a second loop.

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
