use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 120;

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
            description: "Replace one exact occurrence of old_text with new_text in a file. old_text must match exactly once, whitespace included.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_text": {"type": "string"},
                    "new_text": {"type": "string"}
                },
                "required": ["path", "old_text", "new_text"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "bash",
            description: "Run a bash command in the working directory. Returns stdout and stderr combined; long output keeps the last 2000 lines or 50KB.",
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

/// Runs a tool call. `Err` becomes an error result the model sees.
pub fn run(name: &str, input: &Value, cwd: &Path) -> Result<String, String> {
    if let Value::String(raw) = input {
        return Err(format!("tool input was not valid JSON: {raw}"));
    }
    match name {
        "read" => read(cwd, input),
        "write" => write(cwd, input),
        "edit" => edit(cwd, input),
        "bash" => bash(cwd, input),
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
    if old.is_empty() {
        return Err("old_text is empty".into());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    match text.matches(old).count() {
        0 => Err(format!("old_text not found in {}", path.display())),
        1 => {
            std::fs::write(&path, text.replacen(old, new, 1))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(format!("edited {}", path.display()))
        }
        n => Err(format!(
            "old_text matches {n} times in {}; include more context",
            path.display()
        )),
    }
}

fn bash(cwd: &Path, input: &Value) -> Result<String, String> {
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
    let status = loop {
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            timed_out = true;
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

    let mut text = tail(&String::from_utf8_lossy(&out));
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

fn kill(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .status();
    }
    let _ = child.kill();
}

/// Keeps the end of long output, where errors and results usually are.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept = Vec::new();
    let mut bytes = 0;
    for line in lines.iter().rev() {
        if kept.len() == MAX_LINES || bytes + line.len() + 1 > MAX_BYTES {
            break;
        }
        bytes += line.len() + 1;
        kept.push(*line);
    }
    kept.reverse();
    let body = kept.join("\n");
    if kept.len() < lines.len() {
        format!(
            "[showing the last {} of {} lines]\n{body}",
            kept.len(),
            lines.len()
        )
    } else {
        body
    }
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
        )
        .unwrap();
        let dup = run(
            "edit",
            &json!({"path": "a/b.txt", "old_text": "two", "new_text": "2"}),
            &dir,
        );
        assert!(dup.unwrap_err().contains("matches 2 times"));
        run(
            "edit",
            &json!({"path": "a/b.txt", "old_text": "one\n", "new_text": "1\n"}),
            &dir,
        )
        .unwrap();
        let out = run(
            "read",
            &json!({"path": "a/b.txt", "offset": 2, "limit": 1}),
            &dir,
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
            run("bash", &json!({"command": "echo hi; echo err >&2"}), &dir)
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert!(
            run("bash", &json!({"command": "exit 3"}), &dir)
                .unwrap_err()
                .ends_with("[exit code 3]")
        );
        let started = Instant::now();
        let err = run(
            "bash",
            &json!({"command": "sleep 5 | cat", "timeout": 1}),
            &dir,
        )
        .unwrap_err();
        assert!(err.contains("killed after 1s"));
        assert!(started.elapsed() < Duration::from_secs(4));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_unparsed_input() {
        assert!(run("read", &Value::String("{\"pa".into()), Path::new(".")).is_err());
    }

    #[test]
    fn tail_keeps_the_end() {
        let long = (0..3000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let t = tail(&long);
        assert!(t.starts_with("[showing the last 2000 of 3000 lines]"));
        assert!(t.ends_with("2999"));
    }
}
