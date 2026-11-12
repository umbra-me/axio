#!/bin/sh
# Builds and checks axio on Linux in a disposable Rust container: fmt,
# clippy, tests, the terminal checks and the bubblewrap sandbox.
#   sh e2e/linux.sh          (needs Docker; the repository is mounted read-only)
set -eu
R=$(cd "$(dirname "$0")/.." && pwd)
docker run --rm --cpus 4 --memory 4g --security-opt seccomp=unconfined \
  -v "$R:/src:ro" rust:1.98.1-bookworm bash -c '
set -e
apt-get update -qq >/dev/null && apt-get install -y -qq bubblewrap python3 >/dev/null 2>&1
useradd -m u
R=/home/u/axio
cp -r /src $R && rm -rf $R/target && chown -R u /home/u
su u -c "export PATH=/usr/local/cargo/bin:\$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/home/u/.cargo
cd $R && uname -m
cargo fmt --all -- --check && echo fmt ok
cargo clippy --all-targets --locked -- -D warnings 2>&1 | tail -1
cargo test --locked 2>&1 | grep -E \"test result|FAILED|panicked\"
cargo build --release --locked 2>&1 | tail -1
sh e2e/all.sh $R/target/release/axio
mkdir -p /home/u/sbx/h /home/u/sbx/p && cd /home/u/sbx/p && git init -q
python3 $R/e2e/sandbox_linux.py $R/target/release/axio /home/u/sbx/h /home/u/sbx/p"
'
