use crate::message::Block;
use std::path::{Path, PathBuf};

const BASE: &str = "You are a coding agent running in axio, a minimal harness. You help the user by reading files, running commands, editing code and writing new files.

Tools:
- Read: read a file
- Bash: run a shell command (use it for ls, rg, find, git, builds and tests)
- Edit: replace exact text in a file
- Write: create or overwrite a file

Read a file before you edit it. Prefer Edit over Write for changes to existing files. Do not re-read a file after a successful edit. Prefer rg for searching. Only say something is done, fixed or tested when tool output shows it. Keep answers short, and show file paths when you mention files.

Project instructions: AGENTS.md (or CLAUDE.md) files apply to the directory tree that contains them. The ones from the filesystem root down to the working directory are included below. When you work in another directory, check for AGENTS.md files there. Deeper files win over shallower ones, and the user's direct instructions win over all of them.";

/// The media type of an image file axio can attach, by its extension.
pub fn image_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Words split on whitespace, where a backslash keeps the next character
/// (as terminals escape spaces in dropped file paths).
fn words(text: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        let word = out.last_mut().expect("never empty");
        match c {
            '\\' => word.extend(chars.next()),
            c if c.is_whitespace() => {
                if !word.is_empty() {
                    out.push(String::new());
                }
            }
            c => word.push(c),
        }
    }
    out.retain(|w| !w.is_empty());
    out
}

/// Files, images and directories the prompt names as `@path`, as blocks
/// for the model, each with a short note for the user.
pub fn mentions(prompt: &str, cwd: &Path) -> Vec<(Block, String)> {
    const MAX_BYTES: u64 = 256 * 1024;
    // Base64 grows a file by a third, and the API takes images up to 5 MB.
    const MAX_IMAGE_BYTES: u64 = 3_750_000;
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for word in words(prompt) {
        let Some(name) = word.strip_prefix('@') else {
            continue;
        };
        let name = name.trim_end_matches([',', '.', ';', ':', '!', '?', ')', ']', '"', '\'']);
        if name.is_empty() || seen.iter().any(|s| s == name) {
            continue;
        }
        seen.push(name.to_string());
        let path = crate::tools::resolve(cwd, name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let text = |t: String| Block::Text { text: t };
        if meta.is_dir() {
            let mut entries: Vec<String> = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| {
                    let slash = if e.path().is_dir() { "/" } else { "" };
                    format!("{}{slash}", e.file_name().to_string_lossy())
                })
                .collect();
            entries.sort();
            let n = entries.len();
            entries.truncate(200);
            out.push((
                text(format!(
                    "<directory path=\"{name}\">\n{}\n</directory>",
                    entries.join("\n")
                )),
                format!("attached {name}/ · {n} entries"),
            ));
        } else if let Some(media_type) = image_type(&path) {
            if meta.len() > MAX_IMAGE_BYTES {
                out.push((
                    text(format!("[{name} is an image too large to attach]")),
                    format!("{name} is too large to attach (images up to 3.7 MB)"),
                ));
            } else if let Ok(bytes) = std::fs::read(&path) {
                let kb = bytes.len().div_ceil(1024);
                out.push((
                    Block::Image {
                        media_type: media_type.into(),
                        data: crate::crypto::base64(&bytes),
                    },
                    format!("attached {name} · image, {kb} KB"),
                ));
            }
        } else if meta.len() > MAX_BYTES {
            out.push((
                text(format!(
                    "[{name} is {} KB, too large to attach; read the parts you need]",
                    meta.len() / 1024
                )),
                format!("{name} is too large to attach; the model was told to read it"),
            ));
        } else if let Ok(body) = std::fs::read_to_string(&path) {
            let lines = body.lines().count();
            let s = if lines == 1 { "" } else { "s" };
            out.push((
                text(format!("<file path=\"{name}\">\n{body}\n</file>")),
                format!("attached {name} · {lines} line{s}"),
            ));
        }
    }
    out
}

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

pub fn system_prompt(cwd: &Path, skills: Option<&str>) -> String {
    let mut prompt = BASE.to_string();
    if let Some(skills) = skills {
        prompt.push_str("\n\n");
        prompt.push_str(skills);
    }
    let found = files(cwd);
    if found.is_empty() {
        prompt.push_str("\n\nNo AGENTS.md or CLAUDE.md applies to the working directory.");
    }
    for (path, text) in found {
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
    fn mentions_attach_files_and_directories() {
        let dir = std::env::temp_dir().join(format!("axio-mentions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
        let text = |b: &Block| match b {
            Block::Text { text } => text.clone(),
            _ => String::new(),
        };
        let got = mentions(
            "look at @src/a.rs, then @src and @missing.rs or mail me@x.y",
            &dir,
        );
        assert_eq!(got.len(), 2);
        assert_eq!(
            text(&got[0].0),
            "<file path=\"src/a.rs\">\nfn a() {}\n\n</file>"
        );
        assert_eq!(got[0].1, "attached src/a.rs · 1 line");
        assert_eq!(
            text(&got[1].0),
            "<directory path=\"src\">\na.rs\n</directory>"
        );
        assert_eq!(
            mentions("@src/a.rs @src/a.rs", &dir).len(),
            1,
            "each file once"
        );
        std::fs::write(dir.join("Screen shot.PNG"), [0x89, b'P', b'N', b'G']).unwrap();
        let got = mentions(r"see @Screen\ shot.PNG", &dir);
        assert_eq!(
            got[0].0,
            Block::Image {
                media_type: "image/png".into(),
                data: "iVBORw==".into()
            }
        );
        assert_eq!(got[0].1, "attached Screen shot.PNG · image, 1 KB");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn converts_days_to_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_768), (2026, 11, 11));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
