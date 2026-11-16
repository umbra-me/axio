# ACP, install paths and 0.2.0 — 2026-11-16

Owner request: continue axio with four items: ACP mode, daily use, easier
install, and the Windows and MCP sign-in loose ends. The owner then asked to
"finish it off". Pull requests were merged under the standing rule (green local
checks and CI, `--merge`).

## Merged

| PR | Change |
| --- | --- |
| #93 | `axio acp`: Agent Client Protocol v1 on stdio (`src/acp.rs`). Agent setup moved from `run()` into `prepare()` in `src/main.rs`; `Agent::prompt_with` carries attached blocks. `e2e/acp.py`. |
| #94 | `scripts/get.sh` (checks `SHA256SUMS`, `AXIO_DOWNLOAD_URL` for tests, `e2e/get.py`); `Formula/axio.rb` in this repository; `scripts/formula.sh`; after-release step in `docs/releasing.md`. |
| #95 | ACP test client reads whole lines only (Windows flake: a half-written line poisoned the lock). |
| #96 | ACP: paragraph break between replies either side of a tool call (found in the live run). |
| #97 | `chore(release): 0.2.0`. Tag `v0.2.0` on `d8e09a3`. |
| #98 | Homebrew formula for 0.2.0, from the published `SHA256SUMS`. |

## Evidence

- Rust checks on each PR: fmt, clippy `-D warnings`, `cargo test` (271 unit
  tests at #96, plus integration tests). `sh e2e/all.sh`: 149 passed, 0 failed.
  CI green on all five jobs for every PR.
- ACP schema: transcripts decoded with the generated Effect decoders for ACP
  schema v1.21.0 (from the t3code reference checkout,
  `packages/effect-acp/src/_generated/schema-v1.gen.ts`). Stub run: 33
  messages, 0 invalid. Live run: 72 messages, 0 invalid. A deliberately
  corrupted copy was rejected.
- Live ACP session (owner-approved spend, `anthropic/claude-haiku-4-5`): a
  FizzBuzz task in a scratch project. TodoWrite became a plan, Write and Bash
  asked with the right options, the file was written and run, and the turn
  ended `end_turn`. Found and fixed #96.
- Windows (Surface, Windows 11): full test suite and `e2e/acp.py` 13/13
  passed; `acp::` tests 15/15 after #95. Build folder
  `C:\Users\kian-\axio-win` removed.
- Install: `get.sh` installed 0.1.0, 0.1.1 and later 0.2.0 into scratch
  folders. The formula was installed from a temporary local tap on macOS
  arm64, with `brew test` and `brew style` clean, then uninstalled and
  untapped.
- Release run 43868640920: 5 builds plus publish green. All 5 assets matched
  `SHA256SUMS` (Mach-O arm64/x86_64, static musl ELF, PE32+). `axio update`
  moved the owner's install from 0.1.1 to 0.2.0, byte-identical to the asset.

## Side effects

- Setting `HOMEBREW_NO_INSTALL_FROM_API` started a clone of homebrew-core. It
  was stopped and the empty `Taps/homebrew` folder removed.
- `brew uninstall axio` autoremoved the orphaned `bison 3.8.2`. It was
  reinstalled and marked not installed on request. Other autoremoved orphans
  cannot be ruled out, because only the last output line was captured.
- The #94 branch was force-pushed (`--force-with-lease`) after a rebase,
  without asking first.

## Open

- Owner: `axio mcp login mobbin` (browser sign-in).
- `axio acp` has not been tried in a real editor client (Zed, Atrium).
- Windows stays experimental: the interactive terminal is not tested there.
