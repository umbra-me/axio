# Changelog

Notable changes per release. Dates are the release date; the format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[SemVer](https://semver.org/spec/v2.0.0.html) — with the usual `0.x` caveat that
a minor bump may break things.

## [Unreleased]

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
  flags — with per-section salvage and `--explain` for provenance.
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
  asks.
- **An optional sandbox**, Linux only: Landlock, applied before the runtime
  starts and inherited by every command axio spawns. `--sandbox`, or
  `[sandbox] enabled`.

### Known limitations

- **The sandbox is filesystem-only and Linux-only.** It says nothing about the
  network, and on other platforms asking for it is a warning rather than
  confinement. Off by default.
- **The Anthropic path is consistent with the documented wire format but has
  never met the real endpoint.** `scripts/live-check.sh` is what would change
  that.
- `budget.max_usd_per_turn` cannot fire on a provider that reports no prices.
  `--doctor` says so rather than leaving it looking enforced.

[Unreleased]: https://github.com/umbra-me/axio/commits/main
