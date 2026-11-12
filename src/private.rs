//! Files only this user can read. Transcripts, prompt history and tool
//! output can hold anything a tool saw, keys and personal data included.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static LOGS: OnceLock<PathBuf> = OnceLock::new();

/// Creates `path` (and its parents) and makes it private to this user.
pub fn dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn options() -> std::fs::OpenOptions {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut o = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o
}

/// Creates or appends to a file only this user can read.
pub fn append(path: &Path) -> std::io::Result<File> {
    options().create(true).append(true).open(path)
}

/// Creates (or empties) a file only this user can read, open for writing.
pub fn create(path: &Path) -> std::io::Result<File> {
    options().create(true).write(true).truncate(true).open(path)
}

/// Writes a new file only this user can read.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    options()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?
        .write_all(bytes)
}

/// Where tool output logs go: `~/.axio/agent/logs` once set at startup.
pub fn set_logs(dir: PathBuf) {
    let _ = LOGS.set(dir);
}

/// The private folder for tool output logs. Without one set (tests, some
/// child processes) each process gets a fresh folder it creates itself, so
/// no other user can have made it first.
pub fn logs() -> std::io::Result<PathBuf> {
    static FALLBACK: OnceLock<PathBuf> = OnceLock::new();
    if let Some(d) = LOGS.get() {
        dir(d)?;
        return Ok(d.clone());
    }
    if let Some(d) = FALLBACK.get() {
        return Ok(d.clone());
    }
    let d = std::env::temp_dir().join(format!(
        "axio-logs-{}-{}",
        std::process::id(),
        crate::crypto::base64url(&crate::crypto::random(6).map_err(std::io::Error::other)?)
    ));
    // create_dir, not create_dir_all: it fails if someone made it first.
    std::fs::create_dir(&d)?;
    dir(&d)?;
    Ok(FALLBACK.get_or_init(|| d).clone())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn folders_and_files_are_private() {
        let root = std::env::temp_dir().join(format!("axio-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir(&root).unwrap();
        assert_eq!(mode(&root), 0o700, "an existing folder is tightened");
        dir(&root.join("a/b")).unwrap();
        assert_eq!(mode(&root.join("a/b")), 0o700);
        use std::io::Write;
        append(&root.join("h.jsonl"))
            .unwrap()
            .write_all(b"x\n")
            .unwrap();
        assert_eq!(mode(&root.join("h.jsonl")), 0o600);
        write(&root.join("out.log"), b"y").unwrap();
        assert_eq!(mode(&root.join("out.log")), 0o600);
        let logs = logs().unwrap();
        assert_eq!(mode(&logs), 0o700);
        assert_eq!(logs, super::logs().unwrap(), "one folder per process");
    }
}
