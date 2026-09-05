# September 5 change set

## Local profiles, capture attachments and desktop delivery — 2026-09-05

`axio local PROFILE [ARGS…]` hands off to the separately installed `axio-local`
launcher, then starts this executable using the named profile. On macOS/Linux,
the desktop Settings → Hosted agents → Axio Local profile field applies that
profile to new hosted Axio terminals. Native Windows is unsupported for Local;
use its CLI inside WSL. Configure profiles in Local before selecting one.

The command palette's **Start Codex with Capture** picks a JSON attachment
exported by Capture and starts a new hosted Codex terminal in a repository
worktree. The image is supplied with `--image`; the user sends the first prompt.
The importer accepts schema 1 PNG manifests, checks safe filenames, directory
containment, SHA-256, PNG header dimensions, a 32 MiB limit and an 80-million-pixel
limit, then copies the image into app-data `attachments/`. This is header and
integrity validation, not full PNG decoding; the consuming agent decodes it.
There is no automatic upload or existing-session attachment flow.

Desktop CI builds both frontends, tests the app state, checks generated TypeScript
for drift, and checks app-enabled compilation on macOS/Windows; Quota's desktop
check remains Windows-only. Releases add desktop bundles for Apple Silicon,
Intel macOS, Windows and Linux, including a bundled CLI. Installers are staged
flat before artifact upload so checksums and release uploads see ordinary files.
The macOS Intel runner is `macos-15-intel`; CLI builds use the locked dependency set.
The Tauri configuration uses the discovered frontend directory for npm commands.

Source preparation is not a published preview. No tag or release is created by
this change. A local Apple Development-signed app was packaged during the earlier
implementation, but public Developer ID signing/notarisation and installer
acceptance remain separate gates. The complete release workflow, fresh-machine
installs, Local-profile live tool use, and Capture-to-Codex UI flow remain unverified.

Validation on September 5: 837 workspace tests, workspace Clippy and formatting, both desktop frontend builds, and app-enabled macOS compilation passed. The 70 app tests are included in the workspace total.

Final source checks: naming firewall passes. The limits script fails on unchanged
`hosted/agent.rs` (467 lines) and `hosted/appserver.rs` (585 lines), above the
300-line limit; no refactor of those unrelated files is included. Generated
AgentSettings.ts includes ts-rs trailing whitespace; it is preserved rather than
hand-editing generated code.
