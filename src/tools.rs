use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

#[derive(Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// Built-in modules a profile can switch on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modules {
    pub search: bool,
    pub plan: bool,
    pub background: bool,
    pub web: bool,
    pub sandbox: bool,
}

impl Modules {
    pub fn from_names(names: &[&str]) -> Result<Modules, String> {
        let mut m = Modules::default();
        for n in names {
            match *n {
                "search" => m.search = true,
                "plan" => m.plan = true,
                "background" => m.background = true,
                other => {
                    return Err(format!(
                        "unknown module {other}: known are search, plan, background"
                    ));
                }
            }
        }
        Ok(m)
    }

    pub fn names(&self) -> Vec<&'static str> {
        [
            ("search", self.search),
            ("plan", self.plan),
            ("background", self.background),
        ]
        .into_iter()
        .filter_map(|(n, on)| on.then_some(n))
        .collect()
    }
}

/// The core tools plus the modules' tools.
pub fn specs_for(m: Modules) -> Vec<ToolSpec> {
    let mut v = specs();
    if m.search {
        v.extend(crate::search::specs());
    }
    if m.plan {
        v.push(crate::plan::spec());
    }
    if m.web {
        v.push(crate::web::spec());
    }
    if m.background {
        if let Some(bash) = v.iter_mut().find(|t| t.name == "Bash") {
            crate::background::extend_bash(bash);
        }
        v.push(crate::background::stop_spec());
    }
    v
}

/// Runs a core or module tool.
pub fn run_with(
    m: Modules,
    name: &str,
    input: &Value,
    cwd: &Path,
    stop: &mut dyn FnMut() -> bool,
) -> Result<String, String> {
    match name {
        "Grep" if m.search => crate::search::grep(cwd, input, true),
        "Glob" if m.search => crate::search::glob(cwd, input, true),
        "TodoWrite" if m.plan => crate::plan::write(input),
        "Bash" if m.background && input["run_in_background"] == true => {
            crate::background::start(cwd, input)
        }
        "TaskStop" if m.background => crate::background::stop(input),
        "WebFetch" if m.web => crate::web::fetch(input),
        _ => run(name, input, cwd, stop),
    }
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "Read".into(),
            description: "Read a text file. Output is cut at 2000 lines or 50KB; use offset and limit to page through larger files.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "File path, relative to the working directory or absolute"},
                    "offset": {"type": "integer", "description": "1-based line to start from"},
                    "limit": {"type": "integer", "description": "Maximum number of lines"}
                },
                "required": ["file_path"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "Write".into(),
            description: "Write a file, creating parent directories and replacing any existing content.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["file_path", "content"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "Edit".into(),
            description: "Replace exact text in a file. old_string must match exactly once, whitespace included; add surrounding lines to make it unique, or set replace_all to change every occurrence (handy for renames). An empty old_string creates a new file containing new_string.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence (default false)"}
                },
                "required": ["file_path", "old_string", "new_string"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "Bash".into(),
            description: "Run a bash command in the working directory. Returns stdout and stderr combined. Long output keeps its beginning and end, and the result names a log file holding the full output, which you can read or search.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout": {"type": "integer", "description": "Milliseconds before the command is killed (default 120000, at most 600000)"}
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
        "Read" => read(cwd, input),
        "Write" => write(cwd, input),
        "Edit" => edit(cwd, input),
        "Bash" => bash(cwd, input, stop),
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
    let path = resolve(cwd, str_arg(input, "file_path")?);
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
    let path = resolve(cwd, str_arg(input, "file_path")?);
    crate::sandbox::check_write(&path)?;
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
    let path = resolve(cwd, str_arg(input, "file_path")?);
    crate::sandbox::check_write(&path)?;
    let old = str_arg(input, "old_string")?;
    let new = str_arg(input, "new_string")?;
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    if old.is_empty() {
        if path.exists() {
            return Err(format!(
                "{} already exists; give old_string, or use Write to replace it",
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
        0 => Err(format!("old_string not found in {}", path.display())),
        n if n > 1 && !all => Err(format!(
            "old_string matches {n} times in {}; include more context or set replace_all",
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

thread_local! {
    /// The newest line a running command printed, for the interface to show.
    /// Per thread: the command and the interface's polling share one.
    static LATEST_LINE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

pub fn latest_output_line() -> Option<String> {
    LATEST_LINE.with_borrow(|line| (!line.is_empty()).then(|| line.clone()))
}

fn set_latest_line(out: &[u8]) {
    let tail = &out[out.len().saturating_sub(512)..];
    let text = strip_ansi(&String::from_utf8_lossy(tail));
    let line = text
        .split(['\n', '\r'])
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("");
    LATEST_LINE.with_borrow_mut(|latest| line.clone_into(latest));
}

/// Text without terminal colour and cursor sequences.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

fn bash(cwd: &Path, input: &Value, stop: &mut dyn FnMut() -> bool) -> Result<String, String> {
    let command = str_arg(input, "command")?;
    let timeout = Duration::from_millis(
        input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS),
    );
    let mut cmd = crate::sandbox::bash(command);
    cmd.current_dir(cwd)
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
    set_latest_line(b"");
    let status = loop {
        let before = out.len();
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
        }
        if out.len() != before {
            set_latest_line(&out);
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

    set_latest_line(b"");
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
pub(crate) fn kill(child: &mut std::process::Child) {
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
pub(crate) fn shorten(text: &str) -> String {
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
            "Write",
            &json!({"file_path": "a/b.txt", "content": "one\ntwo\ntwo\n"}),
            &dir,
            &mut || false,
        )
        .unwrap();
        let dup = run(
            "Edit",
            &json!({"file_path": "a/b.txt", "old_string": "two", "new_string": "2"}),
            &dir,
            &mut || false,
        );
        assert!(dup.unwrap_err().contains("matches 2 times"));
        run(
            "Edit",
            &json!({"file_path": "a/b.txt", "old_string": "one\n", "new_string": "1\n"}),
            &dir,
            &mut || false,
        )
        .unwrap();
        let out = run(
            "Read",
            &json!({"file_path": "a/b.txt", "offset": 2, "limit": 1}),
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
                "Bash",
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
            run("Bash", &json!({"command": "exit 3"}), &dir, &mut || false)
                .unwrap_err()
                .ends_with("[exit code 3]")
        );
        let started = Instant::now();
        let err = run(
            "Bash",
            &json!({"command": "sleep 5 | cat", "timeout": 1000}),
            &dir,
            &mut || false,
        )
        .unwrap_err();
        assert!(err.contains("killed after 1s"));
        assert!(started.elapsed() < Duration::from_secs(4));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn running_commands_report_their_newest_line() {
        let mut seen = Vec::new();
        bash(
            &std::env::temp_dir(),
            &json!({"command": "printf 'one\\n\\033[32mtwo\\033[0m\\n'; sleep 0.3; printf 'three\\r'; sleep 0.3"}),
            &mut || {
                if let Some(l) = latest_output_line()
                    && seen.last() != Some(&l)
                {
                    seen.push(l);
                }
                false
            },
        )
        .unwrap();
        assert_eq!(seen, ["two", "three"]);
        assert_eq!(latest_output_line(), None, "cleared when the command ends");
    }

    #[test]
    fn bash_stops_when_asked() {
        let dir = tmp();
        let started = Instant::now();
        let mut polls = 0;
        let err = run(
            "Bash",
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
    fn uses_claude_code_tool_names_and_parameters() {
        let specs = specs();
        let shape: Vec<(&str, Vec<&str>)> = specs
            .iter()
            .map(|t| {
                let props = t.schema["properties"].as_object().unwrap();
                (t.name.as_str(), props.keys().map(|k| k.as_str()).collect())
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("Read", vec!["file_path", "limit", "offset"]),
                ("Write", vec!["content", "file_path"]),
                (
                    "Edit",
                    vec!["file_path", "new_string", "old_string", "replace_all"]
                ),
                ("Bash", vec!["command", "timeout"]),
            ]
        );
    }

    #[test]
    fn profiles_choose_the_tool_set() {
        let names = |m: Modules| specs_for(m).into_iter().map(|t| t.name).collect::<Vec<_>>();
        assert_eq!(names(Modules::default()), ["Read", "Write", "Edit", "Bash"]);
        let standard = Modules {
            search: true,
            plan: true,
            background: true,
            ..Modules::default()
        };
        assert_eq!(
            names(standard),
            [
                "Read",
                "Write",
                "Edit",
                "Bash",
                "Grep",
                "Glob",
                "TodoWrite",
                "TaskStop"
            ]
        );
        let bash = specs_for(standard)
            .into_iter()
            .find(|t| t.name == "Bash")
            .unwrap();
        assert!(bash.schema["properties"]["run_in_background"].is_object());
        let off = run_with(
            Modules::default(),
            "Grep",
            &json!({"pattern": "x"}),
            Path::new("."),
            &mut || false,
        );
        assert_eq!(off, Err("unknown tool: Grep".into()));
    }

    #[test]
    fn the_full_profile_adds_web_and_sandbox() {
        let full = crate::profile::load(Path::new("/nonexistent"), "full")
            .unwrap()
            .modules;
        assert!(full.sandbox);
        let names: Vec<String> = specs_for(full).into_iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "Read",
                "Write",
                "Edit",
                "Bash",
                "Grep",
                "Glob",
                "TodoWrite",
                "WebFetch",
                "TaskStop"
            ]
        );
    }

    #[test]
    fn rejects_unparsed_input() {
        assert!(
            run(
                "Read",
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
            "Bash",
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
            "Edit",
            &json!({"file_path": "n/new.txt", "old_string": "", "new_string": "a a"}),
            &dir,
            no,
        )
        .unwrap();
        assert!(
            run(
                "Edit",
                &json!({"file_path": "n/new.txt", "old_string": "", "new_string": "x"}),
                &dir,
                no
            )
            .is_err()
        );
        let out = run(
            "Edit",
            &json!({"file_path": "n/new.txt", "old_string": "a", "new_string": "b", "replace_all": true}),
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
