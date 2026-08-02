//! Tauri's build step, run only when the desktop app is actually being built.
//!
//! `tauri_build::build()` reads `tauri.conf.json` and fails without it, so calling it
//! unconditionally would break every headless build of the workspace. `tauri-build` is
//! also an *optional* build-dependency, so without the feature the crate is not linked at
//! all and naming it is a compile error rather than a runtime one — which is why this is a
//! `cfg` and not an `if`.
//!
//! Cargo compiles build scripts with the package's enabled features, so `cfg(feature)`
//! is available here. Verified both ways: `cargo build -p axio-quota` compiles with the
//! call absent, and `--features app` still produces a working Tauri context.

fn main() {
    #[cfg(feature = "app")]
    tauri_build::build();
}
