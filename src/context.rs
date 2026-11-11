use std::path::{Path, PathBuf};

const BASE: &str = "You are a coding agent running in axio, a minimal harness. You help the user by reading files, running commands, editing code and writing new files.

Tools:
- Read: read a file
- Bash: run a shell command (use it for ls, rg, find, git, builds and tests)
- Edit: replace exact text in a file
- Write: create or overwrite a file

Read a file before you edit it. Prefer Edit over Write for changes to existing files. Do not re-read a file after a successful edit. Prefer rg for searching. Only say something is done, fixed or tested when tool output shows it. Keep answers short, and show file paths when you mention files.

Project instructions: AGENTS.md (or CLAUDE.md) files apply to the directory tree that contains them. The ones from the filesystem root down to the working directory are included below. When you work in another directory, check for AGENTS.md files there. Deeper files win over shallower ones, and the user's direct instructions win over all of them.";

/// The directory holding axio's settings, sessions and global AGENTS.md.
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("AXIO_HOME") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_else(|| ".".into());
    Path::new(&home).join(".axio").join("agent")
}

/// Global AGENTS.md first, then each directory from the filesystem root down
/// to `cwd`. CLAUDE.md is read where a directory has no AGENTS.md.
pub fn files(cwd: &Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let global = home().join("AGENTS.md");
    if let Ok(text) = std::fs::read_to_string(&global) {
        found.push((global, text));
    }
    let mut dirs: Vec<&Path> = cwd.ancestors().collect();
    dirs.reverse();
    for dir in dirs {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = dir.join(name);
            if let Ok(text) = std::fs::read_to_string(&path) {
                found.push((path, text));
                break;
            }
        }
    }
    found
}

pub fn system_prompt(cwd: &Path) -> String {
    let mut prompt = BASE.to_string();
    for (path, text) in files(cwd) {
        prompt.push_str(&format!(
            "\n\n<project-instructions path=\"{}\">\n{}\n</project-instructions>",
            path.display(),
            text.trim()
        ));
    }
    prompt.push_str(&format!(
        "\n\nDate: {}\nWorking directory: {}",
        today(),
        cwd.display()
    ));
    prompt
}

fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's days-to-civil conversion, to avoid a date dependency.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_days_to_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_768), (2026, 11, 11));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
