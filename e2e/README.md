# Terminal checks

`all.sh` drives a built axio in pseudo-terminals, one Python script per
feature, against local stub model servers: no model is called and nothing is
spent. Each script prints `PASS` or `FAIL` lines; `all.sh` prints only the
failures and a count.

```sh
cargo build --release --locked
sh e2e/all.sh target/release/axio
```

The binary must be named `axio`, because `axio-<name>` selects a profile.
Temporary homes go under `$TMPDIR`. Python 3 is the only requirement.

`linux.sh` runs the Rust checks, these terminal checks and
`sandbox_linux.py` (bubblewrap) in a disposable `rust:1.98.1-bookworm`
container, as a non-root user so bwrap works.

Run one full suite at a time; they start many short-lived processes.
