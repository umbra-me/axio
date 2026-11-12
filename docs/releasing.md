# Releasing axio

## Versions

axio uses [Semantic Versioning](https://semver.org) and stays on `0.x` while
the interface is still settling:

- a `fix:` change raises the patch number (0.1.0 → 0.1.1);
- a `feat:` change, or a breaking one, raises the minor number (0.1.1 →
  0.2.0); breaking changes are listed under **Breaking** in the changelog;
- a version with a suffix (`0.2.0-beta.1`) is a prerelease: GitHub marks it
  so, and `axio update` skips it.

`CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Every pull request adds its line under `## Unreleased`.

## Cutting a release

1. Move the `## Unreleased` entries under a new `## [X.Y.Z] - YYYY-MM-DD`
   heading and leave an empty `## Unreleased` above it.
2. Set `version = "X.Y.Z"` in `Cargo.toml` and run `cargo build --offline`
   so `Cargo.lock` follows (`--locked` refuses to write the new version).
3. Check that the notes come out: `sh scripts/release-notes.sh X.Y.Z`.
4. Merge that as `chore(release): X.Y.Z`, then tag the merge commit and push
   the tag:

   ```sh
   git tag -a vX.Y.Z -m "axio X.Y.Z" && git push origin vX.Y.Z
   ```

`.github/workflows/release.yml` then builds one binary per platform, checks
that the tag matches `Cargo.toml`, and publishes a GitHub release with the
changelog section and `SHA256SUMS`:

| Asset | Platform |
| --- | --- |
| `axio-aarch64-apple-darwin` | macOS, Apple silicon |
| `axio-x86_64-apple-darwin` | macOS, Intel |
| `axio-x86_64-unknown-linux-musl` | Linux x86_64 (static) |
| `axio-aarch64-unknown-linux-musl` | Linux arm64 (static) |
| `axio-x86_64-pc-windows-msvc.exe` | Windows x86_64 (experimental) |

`axio update` downloads the asset for its platform from the latest release
and refuses it unless its SHA-256 matches `SHA256SUMS`.
