use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[derive(Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read",
            description: "Read a text file. Output is cut at 2000 lines or 50KB; use offset and limit to page through larger files.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the working directory or absolute"},
                    "offset": {"type": "integer", "description": "1-based line to start from"},
                    "limit": {"type": "integer", "description": "Maximum number of lines"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "write",
            description: "Write a file, creating parent directories and replacing any existing content.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "edit",
            description: "Replace exact text in a file. old_text must match exactly once, whitespace included; add surrounding lines to make it unique, or set replace_all to change every occurrence (handy for renames). An empty old_text creates a new file containing new_text.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_text": {"type": "string"},
                    "new_text": {"type": "string"},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence (default false)"}
                },
                "required": ["path", "old_text", "new_text"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "bash",
            description: "Run a bash command in the working directory. Returns stdout and stderr combined. Long output keeps its beginning and end, and the result names a log file holding the full output, which you can read or search.",
            schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout": {"type": "integer", "description": "Seconds before the command is killed (default 120)"}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
    ]
}

/// Ends the output of a command the user stopped.
pub const STOPPED: &str = "[stopped by the user]";

/// Runs a tool call. `Err` becomes an error result the model sees.
/// `stop` is polled while a command runs.
pub fn run(
    name: &str,
    input: &Value,
    cwd: &Path,
    stop: &mut dyn FnMut() -> bool,
) -> Result<String, String> {
    if let Value::String(raw) = input {
        return Err(format!("tool input was not valid JSON: {raw}"));
    }
    match name {
        "read" => read(cwd, input),
        "write" => write(cwd, input),
        "edit" => edit(cwd, input),
        "bash" => bash(cwd, input, stop),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("missing string argument: {key}"))
}

fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|h| Path::new(&h).join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    };
    cwd.join(expanded)
}

fn read(cwd: &Path, input: &Value) -> Result<String, String> {
    let path = resolve(cwd, str_arg(input, "path")?);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = input["limit"].as_u64().map(|n| n as usize);
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(String::new());
    }
    if offset > lines.len() {
        return Err(format!(
            "offset {offset} is past the end ({} lines)",
            lines.len()
        ));
    }
    let end = limit
        .map(|l| (offset - 1 + l).min(lines.len()))
        .unwrap_or(lines.len());
    let mut out = String::new();
    let mut shown = 0;
    for line in &lines[offset - 1..end] {
        if shown == MAX_LINES || out.len() + line.len() + 1 > MAX_BYTES {
            break;
        }
        out.push_str(line);
        out.push('\n');
        shown += 1;
    }
    let last = offset - 1 + shown;
    if last < lines.len() {
        out.push_str(&format!(
            "\n[showing lines {offset}-{last} of {}; continue with offset {}]",
            lines.len(),
            last + 1
        ));
    }
    Ok(out)
}

fn write(cwd: &Path, input: &Value) -> Result<String, String> {
    let path = resolve(cwd, str_arg(input, "path")?);
    let content = str_arg(input, "content")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "wrote {} bytes to {}",
        content.len(),
        path.display()
    ))
}

fn edit(cwd: &Path, input: &Value) -> Result<String, String> {
    let path = resolve(cwd, str_arg(input, "path")?);
    let old = str_arg(input, "old_text")?;
    let new = str_arg(input, "new_text")?;
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    if old.is_empty() {
        if path.exists() {
            return Err(format!(
                "{} already exists; give old_text, or use write to replace it",
                path.display()
            ));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        std::fs::write(&path, new).map_err(err)?;
        return Ok(format!("created {}", path.display()));
    }
    let text = std::fs::read_to_string(&path).map_err(err)?;
    let all = input["replace_all"].as_bool().unwrap_or(false);
    match text.matches(old).count() {
        0 => Err(format!("old_text not found in {}", path.display())),
        n if n > 1 && !all => Err(format!(
            "old_text matches {n} times in {}; include more context or set replace_all",
            path.display()
        )),
        n => {
            std::fs::write(&path, text.replace(old, new)).map_err(err)?;
            let times = if n > 1 {
                format!(" ({n} replacements)")
            } else {
                String::new()
            };
            Ok(format!("edited {}{times}", path.display()))
        }
    }
}

fn bash(cwd: &Path, input: &Value, stop: &mut dyn FnMut() -> bool) -> Result<String, String> {
    let command = str_arg(input, "command")?;
    let timeout = Duration::from_secs(input["timeout"].as_u64().unwrap_or(DEFAULT_TIMEOUT_SECS));
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        // Own process group, so a timeout kills the whole pipeline.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start bash: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let mut readers = Vec::new();
    for mut pipe in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().unwrap()),
    ] {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);

    let started = Instant::now();
    let mut out = Vec::new();
    let mut timed_out = false;
    let mut stopped = false;
    let status = loop {
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if started.elapsed() >= timeout || stop() {
            timed_out = started.elapsed() >= timeout;
            stopped = !timed_out;
            kill(&mut child);
            break child.wait().ok();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Background jobs can hold the pipes open; stop waiting once the shell is gone.
    let deadline = Instant::now() + Duration::from_millis(200);
    while let Ok(chunk) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        out.extend_from_slice(&chunk);
    }

    let mut text = shorten(&String::from_utf8_lossy(&out));
    if stopped {
        return Err(format!("{text}\n{STOPPED}"));
    }
    if timed_out {
        text.push_str(&format!("\n[killed after {}s]", timeout.as_secs()));
        return Err(text);
    }
    match status.and_then(|s| s.code()) {
        Some(0) => Ok(text),
        Some(code) => Err(format!("{text}\n[exit code {code}]")),
        None => Err(format!("{text}\n[terminated by signal]")),
    }
}

/// SIGTERM to the whole process group, then SIGKILL after a second, so
/// programs get a chance to clean up.
fn kill(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let signal = |sig: &str| {
            let _ = Command::new("kill")
                .args([sig, "--", &group])
                .stderr(Stdio::null())
                .status();
        };
        signal("-TERM");
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline && !matches!(child.try_wait(), Ok(Some(_))) {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Children that ignored TERM, or outlived the shell, still hold the group.
        signal("-KILL");
    }
    let _ = child.kill();
}

/// Long output keeps its first quarter and last three quarters of the budget,
/// where commands usually say what they ran and how they ended. The full text
/// goes to a log file the model can read.
fn shorten(text: &str) -> String {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= MAX_LINES && text.len() <= MAX_BYTES {
        return text.to_string();
    }
    let take = |iter: &mut dyn Iterator<Item = &&str>, max_lines: usize, max_bytes: usize| {
        let mut kept = 0;
        let mut bytes = 0;
        for line in iter {
            if kept == max_lines || bytes + line.len() + 1 > max_bytes {
                break;
            }
            bytes += line.len() + 1;
            kept += 1;
        }
        kept
    };
    let head = take(&mut lines.iter(), MAX_LINES / 4, MAX_BYTES / 4);
    let tail = take(
        &mut lines[head..].iter().rev(),
        MAX_LINES * 3 / 4,
        MAX_BYTES * 3 / 4,
    );
    let omitted = lines.len() - head - tail;
    let saved = save_output(text).map_or_else(
        |e| format!("the full output could not be saved: {e}"),
        |p| format!("full output: {}", p.display()),
    );
    format!(
        "{}\n[… {omitted} lines omitted; {saved} …]\n{}",
        lines[..head].join("\n"),
        lines[lines.len() - tail..].join("\n")
    )
}

fn save_output(text: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("axio");
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!("output-{}-{stamp}.log", std::process::id()));
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "axio-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_edit_read_round_trip() {
        let dir = tmp();
        run(
            "write",
            &json!({"path": "a/b.txt", "content": "one\ntwo\ntwo\n"}),
            &dir,
            &mut || false,
        )
        .unwrap();
        let dup = run(
            "edit",
            &json!({"path": "a/b.txt", "old_text": "two", "new_text": "2"}),
            &dir,
            &mut || false,
        );
        assert!(dup.unwrap_err().contains("matches 2 times"));
        run(
            "edit",
            &json!({"path": "a/b.txt", "old_text": "one\n", "new_text": "1\n"}),
            &dir,
            &mut || false,
        )
        .unwrap();
        let out = run(
            "read",
            &json!({"path": "a/b.txt", "offset": 2, "limit": 1}),
            &dir,
            &mut || false,
        )
        .unwrap();
        assert_eq!(
            out,
            "two\n\n[showing lines 2-2 of 3; continue with offset 3]"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bash_reports_exit_codes_and_timeouts() {
        let dir = tmp();
        assert_eq!(
            run(
                "bash",
                &json!({"command": "echo hi; echo err >&2"}),
                &dir,
                &mut || false
            )
            .unwrap()
            .lines()
            .count(),
            2
        );
        assert!(
            run("bash", &json!({"command": "exit 3"}), &dir, &mut || false)
                .unwrap_err()
                .ends_with("[exit code 3]")
        );
        let started = Instant::now();
        let err = run(
            "bash",
            &json!({"command": "sleep 5 | cat", "timeout": 1}),
            &dir,
            &mut || false,
        )
        .unwrap_err();
        assert!(err.contains("killed after 1s"));
        assert!(started.elapsed() < Duration::from_secs(4));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bash_stops_when_asked() {
        let dir = tmp();
        let started = Instant::now();
        let mut polls = 0;
        let err = run(
            "bash",
            &json!({"command": "echo begun; sleep 5"}),
            &dir,
            &mut || {
                polls += 1;
                polls > 20
            },
        )
        .unwrap_err();
        assert!(err.ends_with(STOPPED), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_unparsed_input() {
        assert!(
            run(
                "read",
                &Value::String("{\"pa".into()),
                Path::new("."),
                &mut || false
            )
            .is_err()
        );
    }

    #[test]
    fn long_output_keeps_both_ends_and_saves_the_rest() {
        let long = (0..3000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let t = shorten(&long);
        assert!(t.starts_with("0\n1\n"));
        assert!(t.ends_with("2999"));
        assert!(t.contains("[… 1000 lines omitted; full output: "));
        let path = t
            .split("full output: ")
            .nth(1)
            .unwrap()
            .split(" …]")
            .next()
            .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), long);
        std::fs::remove_file(path).unwrap();
        assert_eq!(shorten("short"), "short");
    }

    #[test]
    fn stopping_lets_the_command_clean_up_first() {
        let dir = tmp();
        let mut polls = 0;
        let err = run(
            "bash",
            &json!({"command": "trap 'echo cleaned up; exit 0' TERM; sleep 5 & wait"}),
            &dir,
            &mut || {
                polls += 1;
                polls > 20
            },
        )
        .unwrap_err();
        assert!(err.contains("cleaned up"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn edit_replaces_all_and_creates_files() {
        let dir = tmp();
        let no = &mut || false;
        run(
            "edit",
            &json!({"path": "n/new.txt", "old_text": "", "new_text": "a a"}),
            &dir,
            no,
        )
        .unwrap();
        assert!(
            run(
                "edit",
                &json!({"path": "n/new.txt", "old_text": "", "new_text": "x"}),
                &dir,
                no
            )
            .is_err()
        );
        let out = run(
            "edit",
            &json!({"path": "n/new.txt", "old_text": "a", "new_text": "b", "replace_all": true}),
            &dir,
            no,
        )
        .unwrap();
        assert!(out.ends_with("(2 replacements)"));
        assert_eq!(
            std::fs::read_to_string(dir.join("n/new.txt")).unwrap(),
            "b b"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
