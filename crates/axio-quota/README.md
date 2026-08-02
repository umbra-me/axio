# axio-quota

How much of each AI coding provider's limit is left, and when it resets.

Three surfaces over one probe layer:

| Surface | Built by | What it is |
| --- | --- | --- |
| `axio quota` | always | The subcommand. Prints a table, `--json`, or `--diagnose`. |
| `axio-quota` | `--features app` | The desktop app: tray icon, HTML flyout, window. |
| the library | always | Probes, models, config, history. No UI dependencies. |

Provider protocol knowledge was derived by reading
[CodexBar](https://github.com/steipete/codexbar) (MIT). No CodexBar source is
included — see `NOTICE` at the repo root.

## Working on it

From the repo root:

```sh
# The headless half. No frontend, no Tauri, no webview — this is the fast loop.
cargo test -p axio-quota
cargo clippy -p axio-quota --all-targets -- -D warnings
axio quota --diagnose            # where each probe looks for credentials

# The desktop app. The frontend must be built first: `frontendDist` is embedded
# at compile time, so a stale `ui/dist` ships silently.
npm --prefix crates/axio-quota/ui install     # once
npm --prefix crates/axio-quota/ui run build
cargo build --release -p axio-quota --features app
./target/release/axio-quota
```

Seeing a raw provider response is usually the fastest way to understand a
mapping bug:

```sh
AXIO_QUOTA_LOG=axio_quota_core=debug axio quota --provider claude
```

### Watch out

**Tauri decides dev-versus-production from `tauri/custom-protocol`, not from the
cargo profile.** Without it, `tauri-build` emits `cfg(dev)` and even a
`--release` build points the webview at `devUrl` — a connection-refused page
where the app should be. The Tauri CLI adds the feature during `tauri build`; we
build with plain cargo, so the `app` feature names it.

There is no `tauri dev` workflow set up. Frontend changes mean
`npm run build` and a re-link, which is a few seconds; wiring up the CLI and HMR
is a reasonable thing to add and has not been needed yet.

## Layout

```
src/
  providers/       one module per provider; credentials.rs + usage.rs each
  model.rs         RateWindow, UsageSnapshot, Credits — what a probe returns
  json.rs          lenient accessors; read the module doc before adding a probe
  config.rs        %APPDATA%\axio\quota\config.json, CodexBar-compatible
  history.rs       JSONL readings, 45-day retention
  focus.rs         which provider the single tray icon shows  <-- unimplemented
  app/             Tauri: commands, tray, icon rasteriser, state
ui/                React + TypeScript, Vite. Design tokens from apps/site.
icons/make-icon.mjs  regenerates icon.ico; tauri-build requires it
```

## Adding a provider

The cheapest kind is an API key and one endpoint — copy `providers/openrouter.rs`,
change the URL and the field mapping, register it in `providers/mod.rs` and
`ProviderId`. The compiler will point at every place that needs updating; the
test in `providers/mod.rs` fails if an id has no probe.

Bring a real payload. Three times during this port, upstream's code encoded an
assumption that did not hold against live data — positional window labels,
`is_active`, and an RPC method that no longer exists. Parse tests are built from
trimmed, de-identified real responses for that reason, not invented ones.

## Not built yet

- **Cost.** The tab lists the transcript directories a scanner would walk and
  says plainly that it is unbuilt. Everything it needs is already on disk.
- **`focus::tray_focus`.** Which provider the single icon shows when several are
  enabled. Four candidate policies and three `#[ignore]`d spec tests are in the
  file; the placeholder returns the first provider.
- **Adaptive refresh.** Fixed five-minute interval today. Upstream tightens the
  schedule near a reset, which is the right answer.
- **More providers.** Gemini and Factory have local credentials on this machine
  already; Cursor and Augment need browser cookies, which Chrome 127+ App-Bound
  Encryption makes genuinely hard.
