//! The `search` module's Grep and Glob tools, with Claude Code's parameters.
//! Both use ripgrep when it is installed, so `.gitignore` is respected, and
//! fall back to a built-in walk (Glob) or the system grep (Grep).

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const GLOB_LIMIT: usize = 100;
/// Directories the fallback walk never enters.
const SKIP: &[&str] = &[".git", "node_modules", "target", ".venv", "__pycache__"];

pub fn specs() -> Vec<crate::tools::ToolSpec> {
    vec![
        crate::tools::ToolSpec {
            name: "Grep".into(),
            description: "Search file contents with a regular expression (ripgrep syntax). Prefer this over grep or rg in Bash. output_mode is files_with_matches (default), content or count.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string", "description": "File or directory to search (default: working directory)"},
                    "glob": {"type": "string", "description": "Only files matching this glob, e.g. *.rs or src/**/*.ts"},
                    "output_mode": {"type": "string", "enum": ["content", "files_with_matches", "count"]},
                    "-i": {"type": "boolean", "description": "Case-insensitive"},
                    "-n": {"type": "boolean", "description": "Line numbers in content mode (default true)"},
                    "-C": {"type": "integer", "description": "Lines of context in content mode"},
                    "head_limit": {"type": "integer", "description": "Keep only the first N lines of output"}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        crate::tools::ToolSpec {
            name: "Glob".into(),
            description: "Find files by name pattern, such as **/*.rs or src/**/test_*.py. Returns paths, most recently changed first.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string", "description": "Directory to search (default: working directory)"}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
    ]
}

fn skip_globs(cmd: &mut Command) {
    for d in SKIP {
        cmd.args(["--glob", &format!("!{d}")]);
    }
}

fn ripgrep() -> bool {
    Command::new("rg")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn base(cwd: &Path, input: &Value) -> PathBuf {
    input["path"]
        .as_str()
        .map_or_else(|| cwd.to_path_buf(), |p| cwd.join(p))
}

/// Glob matching for `*`, `**`, `?` and `{a,b}` against a `/`-separated path.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    if let Some(open) = pattern.find('{')
        && let Some(close) = pattern[open..].find('}')
    {
        let (head, rest) = (&pattern[..open], &pattern[open + close + 1..]);
        return pattern[open + 1..open + close]
            .split(',')
            .any(|alt| glob_match(&format!("{head}{alt}{rest}"), path));
    }
    // A pattern without a slash matches the file name anywhere, as rg does.
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return segment(pattern.as_bytes(), name.as_bytes());
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    segments(&p, &s)
}

fn segments(p: &[&str], s: &[&str]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((&"**", rest)) => (0..=s.len()).any(|i| segments(rest, &s[i..])),
        Some((first, rest)) => {
            !s.is_empty() && segment(first.as_bytes(), s[0].as_bytes()) && segments(rest, &s[1..])
        }
    }
}

fn segment(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => segment(&p[1..], s) || (!s.is_empty() && segment(p, &s[1..])),
        (Some(b'?'), Some(_)) => segment(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => segment(&p[1..], &s[1..]),
        _ => false,
    }
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name();
        if path.is_dir() {
            if !SKIP.iter().any(|s| name == *s) {
                walk(&path, root, out);
            }
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
}

pub fn glob(cwd: &Path, input: &Value, use_rg: bool) -> Result<String, String> {
    let pattern = input["pattern"]
        .as_str()
        .ok_or("missing string argument: pattern")?;
    let root = base(cwd, input);
    let mut files: Vec<PathBuf> = if use_rg && ripgrep() {
        let mut cmd = Command::new("rg");
        cmd.args(["--files", "--hidden"]);
        skip_globs(&mut cmd);
        let out = cmd
            .args(["--glob", pattern])
            .current_dir(&root)
            .output()
            .map_err(|e| format!("rg failed: {e}"))?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(PathBuf::from)
            .collect()
    } else {
        let mut all = Vec::new();
        walk(&root, &root, &mut all);
        all.into_iter()
            .filter(|p| glob_match(pattern, &p.to_string_lossy()))
            .collect()
    };
    let modified = |p: &PathBuf| {
        std::fs::metadata(root.join(p))
            .and_then(|m| m.modified())
            .ok()
    };
    files.sort_by(|a, b| modified(b).cmp(&modified(a)).then(a.cmp(b)));
    if files.is_empty() {
        return Ok("no files found".into());
    }
    let total = files.len();
    let mut text: Vec<String> = files
        .iter()
        .take(GLOB_LIMIT)
        .map(|p| root.join(p).display().to_string())
        .collect();
    if total > GLOB_LIMIT {
        text.push(format!("[{} more; narrow the pattern]", total - GLOB_LIMIT));
    }
    Ok(text.join("\n"))
}

pub fn grep(cwd: &Path, input: &Value, use_rg: bool) -> Result<String, String> {
    let pattern = input["pattern"]
        .as_str()
        .ok_or("missing string argument: pattern")?;
    let path = base(cwd, input);
    let mode = input["output_mode"]
        .as_str()
        .unwrap_or("files_with_matches");
    let insensitive = input["-i"].as_bool().unwrap_or(false);
    let numbers = input["-n"].as_bool().unwrap_or(true);
    let context = input["-C"].as_u64();
    let mut cmd;
    if use_rg && ripgrep() {
        cmd = Command::new("rg");
        cmd.args(["--hidden", "--color", "never"]);
        skip_globs(&mut cmd);
        if let Some(g) = input["glob"].as_str() {
            cmd.args(["--glob", g]);
        }
        match mode {
            "content" => {
                cmd.arg(if numbers { "-n" } else { "-N" });
                if let Some(c) = context {
                    cmd.arg(format!("-C{c}"));
                }
            }
            "count" => {
                cmd.arg("--count-matches");
            }
            _ => {
                cmd.arg("-l");
            }
        }
        if insensitive {
            cmd.arg("-i");
        }
        cmd.arg("--").arg(pattern).arg(&path);
    } else {
        cmd = Command::new("grep");
        cmd.arg("-rIE");
        for d in SKIP {
            cmd.arg(format!("--exclude-dir={d}"));
        }
        if let Some(g) = input["glob"].as_str() {
            cmd.arg(format!("--include={}", g.rsplit('/').next().unwrap_or(g)));
        }
        match mode {
            "content" => {
                if numbers {
                    cmd.arg("-n");
                }
                if let Some(c) = context {
                    cmd.arg(format!("-C{c}"));
                }
            }
            "count" => {
                cmd.arg("-c");
            }
            _ => {
                cmd.arg("-l");
            }
        }
        if insensitive {
            cmd.arg("-i");
        }
        cmd.arg("-e").arg(pattern).arg(&path);
    }
    let out = cmd
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("search failed to start: {e}"))?;
    // Exit 1 means no matches for both rg and grep; 2 is an error.
    if out.status.code() == Some(2) {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let mut lines: Vec<&str> = std::str::from_utf8(&out.stdout)
        .unwrap_or("")
        .lines()
        .filter(|l| !(mode == "count" && l.ends_with(":0")))
        .collect();
    if lines.is_empty() {
        return Ok("no matches".into());
    }
    if mode == "files_with_matches" {
        lines.sort_unstable();
    }
    let total = lines.len();
    if let Some(limit) = input["head_limit"].as_u64() {
        lines.truncate(limit as usize);
    }
    let mut text = lines.join("\n");
    if lines.len() < total {
        text.push_str(&format!("\n[{} more lines]", total - lines.len()));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_like_ripgrep() {
        assert!(glob_match("*.rs", "src/main.rs"));
        assert!(glob_match("**/*.rs", "main.rs") && glob_match("**/*.rs", "a/b/c.rs"));
        assert!(glob_match("src/**/test_*.py", "src/x/y/test_a.py"));
        assert!(!glob_match("src/*.rs", "src/a/b.rs"));
        assert!(glob_match("*.{ts,tsx}", "ui/App.tsx") && !glob_match("*.{ts,tsx}", "a.js"));
        assert!(glob_match("a?c", "abc") && !glob_match("a?c", "ac"));
    }

    fn tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "axio-search-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (p, t) in [
            ("src/main.rs", "fn main() {\n    println!(\"Hello\");\n}\n"),
            ("src/lib.rs", "pub fn hello() {}\n"),
            ("README.md", "hello world\n"),
            ("node_modules/x/index.js", "hello\n"),
        ] {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        }
        root
    }

    #[test]
    fn glob_and_grep_agree_with_and_without_ripgrep() {
        let root = tree();
        for rg in [false, true] {
            let found = glob(&root, &json!({"pattern": "**/*.rs"}), rg).unwrap();
            assert_eq!(found.lines().count(), 2, "rg={rg}: {found}");
            let files = grep(&root, &json!({"pattern": "hello", "-i": true}), rg).unwrap();
            let names: Vec<&str> = files
                .lines()
                .map(|l| l.rsplit(['/', '\\']).next().unwrap())
                .collect();
            assert_eq!(names, ["README.md", "lib.rs", "main.rs"], "rg={rg}");
            let content = grep(
                &root,
                &json!({"pattern": "Hello", "output_mode": "content", "glob": "*.rs"}),
                rg,
            )
            .unwrap();
            assert!(
                content.ends_with("main.rs:2:    println!(\"Hello\");"),
                "rg={rg}: {content}"
            );
            assert_eq!(
                grep(&root, &json!({"pattern": "zzz"}), rg).unwrap(),
                "no matches"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
