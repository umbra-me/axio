# apps/site

The axio website. Two static files and no build step.

```sh
python3 -m http.server -d apps/site 8080   # then open http://localhost:8080
```

## Why there is no toolchain here

This is a Rust repository. Its CI runs `scripts/firewall.sh`,
`scripts/limits.sh`, `cargo fmt`, clippy and the test matrix, and nothing here
should add a second toolchain to that. A generator would bring a lockfile, a
dependency tree and a version to keep current, in exchange for templating a page
that does not yet need it.

`limits.sh` caps workspace members at four and counts Rust LOC per crate under
`crates/`. Nothing in this directory is a Cargo member or a `.rs` file, so the
gates do not see it — but `firewall.sh` greps the whole tracked tree, so
anything written here is subject to it like any other file.

## Content rule

Every claim on the page is either in the README or in the code. Where the two
disagreed, the code won:

- `crates/axio-core/src/auth.rs:52` declares four providers. The README's
  Providers section still says "three names over two implementations", written
  before `openai-codex` landed. The page says four names over three dialects.

The verification table is the part most likely to go stale, because it records
what has been run against a live endpoint rather than what exists. Update it
when a transport meets its endpoint or a release is cut — a page that claims a
path is unverified after it has been verified is a smaller problem than the
reverse, but both are wrong.

## Deployment

Not wired up yet. When it is, it is served as static files behind the Umbra
control plane like any other per-site project; the site needs no runtime of its
own.
