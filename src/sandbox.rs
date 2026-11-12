//! The `sandbox` module: Bash commands may write only inside the project and
//! temporary folders, and reach the network only when allowed. macOS uses
//! Seatbelt (`sandbox-exec`), Linux uses bubblewrap (`bwrap`). Write and Edit
//! follow the same write limits.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq)]
pub struct Sandbox {
    /// Folders commands may write in, fully resolved.
    pub writable: Vec<PathBuf>,
    pub network: bool,
    /// Folders kept unwritable even inside a writable one: a read-only
    /// sub-agent's project when it sits in a temporary folder.
    pub protected: Vec<PathBuf>,
}

static ACTIVE: OnceLock<Sandbox> = OnceLock::new();

/// Whether this platform has a sandbox at all (Windows has none).
pub fn supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}

/// Turns the sandbox on for this process; called once at startup.
pub fn activate(s: Sandbox) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        if !Path::new("/usr/bin/sandbox-exec").exists() {
            return Err("the sandbox module needs /usr/bin/sandbox-exec".into());
        }
    } else if cfg!(target_os = "linux") {
        let found = Command::new("bwrap")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !found {
            return Err("the sandbox module needs bubblewrap (bwrap) on Linux".into());
        }
    } else {
        return Err("the sandbox module runs on macOS and Linux only".into());
    }
    let _ = ACTIVE.set(s);
    Ok(())
}

pub fn active() -> Option<&'static Sandbox> {
    ACTIVE.get()
}

fn real(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

impl Sandbox {
    /// The project root plus the temporary folders.
    pub fn for_project(root: &Path, network: bool) -> Sandbox {
        let mut s = Sandbox::read_only(root);
        s.protected.clear();
        s.writable.insert(0, real(root));
        s.writable.dedup();
        s.network = network;
        s
    }

    /// Only the temporary folders, never the project at `root`, and no
    /// network.
    pub fn read_only(root: &Path) -> Sandbox {
        let mut writable = vec![real(&std::env::temp_dir())];
        if cfg!(target_os = "macos") {
            writable.push(PathBuf::from("/private/tmp"));
            writable.push(PathBuf::from("/private/var/folders"));
        } else {
            writable.push(PathBuf::from("/tmp"));
        }
        writable.dedup();
        Sandbox {
            writable,
            network: false,
            protected: vec![real(root)],
        }
    }

    /// Whether Write or Edit may change `path`. A file that does not exist
    /// yet is judged by its nearest existing parent.
    pub fn allows_write(&self, path: &Path) -> bool {
        let mut probe = path.to_path_buf();
        let resolved = loop {
            if let Ok(r) = probe.canonicalize() {
                break r.join(path.strip_prefix(&probe).unwrap_or(Path::new("")));
            }
            if !probe.pop() {
                return false;
            }
        };
        self.writable.iter().any(|w| resolved.starts_with(w))
            && !self.protected.iter().any(|p| resolved.starts_with(p))
    }

    pub fn seatbelt_profile(&self) -> String {
        let quote = |p: &Path| {
            p.display()
                .to_string()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        };
        let roots: Vec<String> = self
            .writable
            .iter()
            .map(|p| format!("(subpath \"{}\")", quote(p)))
            .collect();
        let mut p = format!(
            "(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write* {} (literal \"/dev/null\") (literal \"/dev/tty\") (literal \"/dev/dtracehelper\") (regex #\"^/dev/fd/\"))\n",
            roots.join(" ")
        );
        // Later rules win, so these override the writable folders above.
        for k in &self.protected {
            p.push_str(&format!("(deny file-write* (subpath \"{}\"))\n", quote(k)));
        }
        if !self.network {
            p.push_str("(deny network-outbound (remote ip))\n");
        }
        p
    }

    /// `bash -c script`, wrapped for this platform.
    pub fn command(&self, script: &str) -> Command {
        if cfg!(target_os = "macos") {
            let mut c = Command::new("/usr/bin/sandbox-exec");
            c.arg("-p")
                .arg(self.seatbelt_profile())
                .args(["bash", "-c", script]);
            c
        } else {
            let mut c = Command::new("bwrap");
            c.args([
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--die-with-parent",
            ]);
            for w in &self.writable {
                c.arg("--bind").arg(w).arg(w);
            }
            // Mounted after the writable folders, so it covers them.
            for k in &self.protected {
                c.arg("--ro-bind").arg(k).arg(k);
            }
            if !self.network {
                c.arg("--unshare-net");
            }
            c.args(["bash", "-c", script]);
            c
        }
    }
}

/// The bash to run: `AXIO_BASH` when set. On Windows, Git Bash, never the
/// `bash.exe` in System32, which runs commands inside WSL instead of on
/// Windows.
pub fn bash_program() -> PathBuf {
    if let Some(p) = std::env::var_os("AXIO_BASH") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        let mut roots: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect();
        roots.push(PathBuf::from(r"C:\Program Files"));
        for root in roots {
            for rel in [r"Git\bin\bash.exe", r"Programs\Git\bin\bash.exe"] {
                let p = root.join(rel);
                if p.is_file() {
                    return p;
                }
            }
        }
        // Git for Windows' bash anywhere on PATH, before WSL's launcher.
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                let p = dir.join("bash.exe");
                let wsl = dir
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(r"\windows\system32");
                if p.is_file() && !wsl && !dir.to_string_lossy().contains("WindowsApps") {
                    return p;
                }
            }
        }
    }
    PathBuf::from("bash")
}

/// The command Bash runs: sandboxed when the module is on.
pub fn bash(script: &str) -> Command {
    match active() {
        Some(s) => s.command(script),
        None => {
            let mut c = Command::new(bash_program());
            c.args(["-c", script]);
            c
        }
    }
}

/// Refuses a Write or Edit outside the writable folders.
pub fn check_write(path: &Path) -> Result<(), String> {
    match active() {
        Some(s) if !s.allows_write(path) => Err(format!(
            "{} is outside the sandbox's writable folders ({})",
            path.display(),
            s.writable
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("axio-sandbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        real(&d)
    }

    #[test]
    fn judges_writes_by_resolved_path() {
        let root = project("paths");
        let s = Sandbox {
            writable: vec![root.clone()],
            network: false,
            protected: Vec::new(),
        };
        assert!(
            s.allows_write(&root.join("src/new/file.rs")),
            "new files under the root"
        );
        assert!(!s.allows_write(&root.join("../escape.txt")));
        assert!(!s.allows_write(Path::new("/etc/hosts")));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", root.join("link")).unwrap();
            assert!(
                !s.allows_write(&root.join("link/hosts")),
                "symlinks out are caught"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_limits_writes_and_network() {
        let root = project("seatbelt");
        let s = Sandbox::for_project(&root, false);
        let outside = std::env::var("HOME").unwrap() + "/axio-sandbox-test-escape";
        let out = s
            .command(&format!(
                "echo in > inside.txt && echo inside-ok; echo x > '{outside}' 2>/dev/null || echo outside-denied; curl -s -m 5 -o /dev/null https://example.com && echo net-open || echo net-blocked"
            ))
            .current_dir(&root)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            ["inside-ok", "outside-denied", "net-blocked"]
        );
        assert!(!Path::new(&outside).exists());
        assert!(
            !Sandbox::for_project(&root, true)
                .seatbelt_profile()
                .contains("network")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Commands for a read-only sub-agent, run in `root`: a write in the
    /// project, then one in another temporary folder.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn read_only_probe(root: &Path) -> Vec<String> {
        let other = std::env::temp_dir().join(format!("axio-ro-other-{}", std::process::id()));
        let out = Sandbox::read_only(root)
            .command(&format!(
                "echo x > inside.txt 2>/dev/null && echo inside-written || echo inside-denied; echo t > '{}' && echo tmp-ok",
                other.display()
            ))
            .current_dir(root)
            .output()
            .unwrap();
        let _ = std::fs::remove_file(other);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn read_only_protects_a_project_inside_a_temporary_folder() {
        let root = project("ro");
        let s = Sandbox::read_only(&root);
        assert!(!s.allows_write(&root.join("src/main.rs")));
        assert!(s.allows_write(&std::env::temp_dir().join("elsewhere.txt")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_read_only_keeps_the_project_unchanged() {
        let root = project("ro-seatbelt");
        assert_eq!(read_only_probe(&root), ["inside-denied", "tmp-ok"]);
        assert!(!root.join("inside.txt").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bwrap_read_only_keeps_the_project_unchanged() {
        if Command::new("bwrap").arg("--version").output().is_err() {
            return;
        }
        let root = project("ro-bwrap");
        assert_eq!(read_only_probe(&root), ["inside-denied", "tmp-ok"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
