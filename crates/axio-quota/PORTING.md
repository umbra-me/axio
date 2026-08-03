# Porting CodexBar to Windows

What was carried across from [CodexBar](https://github.com/steipete/codexbar), what was
deliberately left behind, and what is still open. Attribution for the knowledge itself is
in the repository `NOTICE`; this is the engineering record behind it.

Written 2026-08-02 against the standalone prototype, before this crate moved into the axio
workspace. The history below is preserved as written; the status sections at the end were
updated when it moved.

## The shape of the original

| Target | Files | Lines | Portable? |
|---|---|---|---|
| `CodexBarCore` | 519 | 143k | mostly — already builds on Linux |
| `CodexBar` (app) | 403 | 82k | no — 135 AppKit + 98 SwiftUI imports |
| `CodexBarCLI` | 33 | 12.5k | yes — ships as Linux glibc and musl tarballs |

Upstream's Linux support is a poor predictor of Windows effort. `CodexBarCore` contains
**219 `#if os(macOS)` blocks against 15 `#if os(Linux)`** — the Linux branches are
fallbacks bolted on for the headless CLI, not a platform abstraction. There are no
`os(Windows)` branches at all.

## Ported, and what changed

**Codex** — from `CodexOAuthUsageFetcher` + `CodexOAuthCredentialsStore`.

Upstream has two paths to the same numbers. `CodexStatusProbe` drives the `codex` TUI in a
pseudo-terminal, sends `/status`, and regex-scrapes the rendered output. The OAuth path
reads `auth.json` and calls the usage API. We ported the second: the PTY path would need
ConPTY and re-breaks whenever the TUI's layout shifts. The `chatgpt_base_url` override in
`config.toml` came along too — enterprise proxy users would otherwise send their bearer
token to the wrong host.

**Claude** — from `ClaudeOAuthUsageFetcher` + `ClaudeOAuthCredentialModels`.

Upstream reads the token from the macOS Keychain and treats `.credentials.json` as a
fallback. On Windows the file is the only store, so the fallback became the primary path.
Both the flat `five_hour` / `seven_day_*` keys and the newer `limits` array are read,
because accounts see one or the other depending on rollout.

One thing was **not** carried across. Upstream sends `User-Agent: claude-code/<version>`
to the usage endpoint, detecting the installed CLI's version by running `claude --version`
and falling back to a hardcoded string. The comment around it reads as though the endpoint
requires a Claude Code-shaped header. Tested against a live account on 2026-08-02, it does
not: `axio-quota/<version>` gets a byte-identical response. So we send our own name, and
`scripts/firewall.sh` fails the build if another client's identifier reappears in
`crates/`. Speaking a provider's protocol is fine; presenting as their software is a
different thing, and it was one line away from being inherited without anyone deciding it.

**Defensive decoding** — the part most worth stealing.

`CodexUsageResponse.init(from:)` wraps every field in `try?`, decodes
`additional_rate_limits` element-by-element so one malformed entry cannot discard its
siblings, and accepts `Double | Int | String` for the same numeric field. That is years of
scar tissue from providers changing shapes underneath the app. Ported as `json.rs`:
payloads parse to `Value` and fields are picked leniently. A live payload validated this
within an hour of writing it — `"balance": "0"` arrives as a string.

## Left behind

| Upstream | Why |
|---|---|
| AppKit/SwiftUI UI (82k lines) | No port path. The Windows tray is a rewrite. |
| Keychain (`Security.framework`) | Windows equivalent is Credential Manager / DPAPI. |
| `posix_spawn`, `TTYCommandRunner` | Needs ConPTY. Avoided by using API paths instead. |
| Sparkle, KeyboardShortcuts, ServiceManagement, Vortex, Swift Charts | Apple-only. |
| 79 of 82 providers | Scope. Add the ones you use. |

## Windows-specific deltas

These were written for a raw Win32 tray. The surface shipped as a Tauri process instead
(see `src/app/`), which wraps the same `Shell_NotifyIconW` — so the constraints held, and
`src/app/icon.rs` exists because of the first one.

- **The tray icon cannot contain text.** macOS status items render "23%" directly;
  `Shell_NotifyIconW` takes a 16x16 `HICON`. The percentage must be drawn into a bitmap on
  every refresh, so the icon renderer is a real component rather than a resource file.
- **Per-monitor DPI** means 16x16 is only correct at 100% scaling. Re-render on DPI change.
- **Explorer restarts** destroy the notification area. The icon must be re-added on the
  `TaskbarCreated` message or it silently vanishes until reboot.
- **Claude's token sits in a plaintext file** protected only by NTFS ACLs. Upstream gets
  Keychain protection for free on macOS. If axio quota ever caches a token itself, that cache
  belongs in Credential Manager, not next to the config.

## Cookie-based providers

Upstream uses `SweetCookieKit` to decrypt browser cookie stores. On Windows, Chrome's
cookie DB is AES-GCM encrypted with a key wrapped by DPAPI — tractable on its own. But
since Chrome 127, **App-Bound Encryption** binds that key to the browser executable
through an elevation service, specifically to stop other processes on the same machine
from reading it. Any approach here should be assumed to break on Chrome updates.

The note this section originally ended on — *"worth checking whether Cursor and Augment
expose a token or API key in their own local config, which would sidestep the browser
entirely"* — is what actually shipped for Cursor: see `src/providers/cursor_local.rs`,
which reads Cursor's own store, alongside `src/providers/cursor.rs` and the sign-in window
in `src/app/connect.rs`. Augment has no provider yet.

## Status of the original roadmap

| # | Item | Now |
|---|---|---|
| 1 | Win32 tray: `Shell_NotifyIconW`, icon bitmap, context menu, `TaskbarCreated` | Shipped, as a Tauri surface rather than raw Win32 — `src/app/tray.rs`, `src/app/icon.rs` |
| 2 | Refresh loop with per-provider backoff, honouring `ProbeError::needs_user_action` | Shipped — `src/app/schedule.rs`. Refreshes arrive from four places and must be throttled or the vendor returns 429 |
| 3 | Settings UI, or keep config file-only | Shipped — the React frontend under `ui/`, `src/app/view.rs` |
| 4 | Token refresh for Claude | **Still open.** The probe reports expiry and tells you to run `claude`; it does not refresh |
| 5 | Cookie providers | Cursor done via its local store; Augment not started |
| 6 | More API-key providers | Several added — `deepseek`, `grok`, `ollama`, `openrouter`, `xai`, `zai`. Copy one, change the URL and the mapping |
