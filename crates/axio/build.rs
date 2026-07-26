//! Stamp the build with a commit sha, when there is one.
//!
//! A published crate has no `.git`, so this degrades to "unknown" rather than
//! failing the build.

use std::process::Command;

fn main() {
    // `.git/HEAD` holds the name of the branch, not its commit, so committing
    // on the branch you are already on does not change it. Watching only that
    // file leaves the binary reporting the sha it was first built at — a stale
    // answer to the one question `--version` exists to answer. The ref itself
    // is what moves, so watch that too, and `packed-refs` for the repository
    // where the loose ref has been packed away.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
    if let Some(reference) = std::fs::read_to_string("../../.git/HEAD")
        .ok()
        .and_then(|head| head.strip_prefix("ref: ").map(|r| r.trim().to_owned()))
    {
        let path = format!("../../.git/{reference}");
        if std::path::Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
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
