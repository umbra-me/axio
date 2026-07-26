//! Stamp the build with a commit sha, when there is one.
//!
//! A published crate has no `.git`, so this degrades to "unknown" rather than
//! failing the build.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-env-changed=AXIO_BUILD_SHA");

    let sha = std::env::var("AXIO_BUILD_SHA").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    });

    println!(
        "cargo:rustc-env=AXIO_BUILD_SHA={}",
        sha.unwrap_or_else(|| "unknown".to_owned())
    );
}
