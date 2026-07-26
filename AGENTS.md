# AGENTS.md

## Commands

None yet — no build system is committed. Add the exact build, test, and lint
invocations here as soon as the first one exists.

## Architecture

Nothing is implemented yet. The intended shape is four crates — `axio-core`
(conversation state, turn loop, `Tool` and `Provider` traits), `axio-provider`
(provider HTTP client and streaming), `axio-tools` (file and shell tools), and
`axio` (the binary, serving both the one-shot CLI and the TUI).

The invariant that makes multiple surfaces cheap: the core emits a stream of
agent events and never renders anything itself. No surface calls a provider or
a tool directly. Turn cancellation is part of the core API, not a later
addition.

This is a rewrite of an earlier implementation, now archived and read-only.
Consult it for prior art — its `docs/gotchas.md` is the highest-value part —
but treat nothing in it as a constraint here.

## Gotchas

None recorded yet. Add only the non-obvious ones.

## Definition of done

- The relevant build and tests run clean, and their output was read.
- Anything unverified is labelled as such.
- Docs invalidated by the change are updated in the same change.
