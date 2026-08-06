//! Tauri's build step, run only when the desktop shell is actually being built.
//!
//! `tauri_build::build()` reads `tauri.conf.json` and fails without it, so an
//! unconditional call would break every headless build of the workspace.
//! `tauri-build` is also an optional build-dependency, so without the feature
//! the crate is not linked at all and naming it is a compile error rather than
//! a runtime one — which is why this is a `cfg` and not an `if`.

fn main() {
    #[cfg(feature = "app")]
    tauri_build::build();
}
