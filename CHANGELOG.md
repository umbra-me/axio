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
