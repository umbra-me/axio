//! The `background` module: `Bash` with `run_in_background` starts a command
//! that outlives the call. Output goes to a log file the model reads with
//! `Read`; `TaskStop` ends it; the model is told when it exits.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Mutex;

struct Task {
    id: String,
    command: String,
    log: PathBuf,
    child: Child,
    reported: bool,
}

static TASKS: Mutex<Vec<Task>> = Mutex::new(Vec::new());

pub fn stop_spec() -> crate::tools::ToolSpec {
    crate::tools::ToolSpec {
        name: "TaskStop".into(),
        description: "Stop a background command started with Bash run_in_background.".into(),
        schema: json!({
            "type": "object",
            "properties": {"task_id": {"type": "string"}},
            "required": ["task_id"],
            "additionalProperties": false
        }),
    }
}

/// Adds `run_in_background` to the Bash tool.
pub fn extend_bash(spec: &mut crate::tools::ToolSpec) {
    spec.schema["properties"]["run_in_background"] = json!({
        "type": "boolean",
        "description": "Start the command and return at once, for dev servers, watchers and long builds"
    });
    spec.description.push_str(" With run_in_background, the command keeps running after the call: you get a task id and a log file to Read, TaskStop ends it, and you are told when it exits.");
}

pub fn start(cwd: &Path, input: &Value) -> Result<String, String> {
    let command = input["command"]
        .as_str()
        .ok_or("missing string argument: command")?;
    let dir = crate::private::logs().map_err(|e| e.to_string())?;
    let mut tasks = TASKS.lock().map_err(|_| "task list poisoned")?;
    let id = format!("bg{}", tasks.len() + 1);
    let log = dir.join(format!("{id}-{}.log", std::process::id()));
    let out = crate::private::append(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = crate::sandbox::bash(command);
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not start bash: {e}"))?;
    let text = format!(
        "Started {id} in the background.\nOutput: {} (read it with Read). TaskStop with task_id {id} ends it; you will be told when it exits.",
        log.display()
    );
    tasks.push(Task {
        id,
        command: command.to_string(),
        log,
        child,
        reported: false,
    });
    Ok(text)
}

pub fn stop(input: &Value) -> Result<String, String> {
    let id = input["task_id"]
        .as_str()
        .ok_or("missing string argument: task_id")?;
    let mut tasks = TASKS.lock().map_err(|_| "task list poisoned")?;
    let task = tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("no background task {id}"))?;
    if matches!(task.child.try_wait(), Ok(Some(_))) {
        return Ok(format!(
            "{id} had already exited; output: {}",
            task.log.display()
        ));
    }
    crate::tools::kill(&mut task.child);
    let _ = task.child.wait();
    task.reported = true;
    Ok(format!("stopped {id}; output: {}", task.log.display()))
}

/// Notes for tasks that exited since the last call, each reported once.
pub fn finished() -> Vec<String> {
    let Ok(mut tasks) = TASKS.lock() else {
        return Vec::new();
    };
    tasks
        .iter_mut()
        .filter(|t| !t.reported)
        .filter_map(|t| {
            let status = t.child.try_wait().ok()??;
            t.reported = true;
            let how = status
                .code()
                .map_or("was killed by a signal".to_string(), |c| {
                    format!("exited with code {c}")
                });
            Some(format!(
                "[Background task {} (`{}`) {how}. Output: {}]",
                t.id,
                t.command,
                t.log.display()
            ))
        })
        .collect()
}

/// Ends every task still running, when axio exits.
pub fn stop_all() {
    if let Ok(mut tasks) = TASKS.lock() {
        for t in tasks.iter_mut() {
            if matches!(t.child.try_wait(), Ok(None)) {
                crate::tools::kill(&mut t.child);
                let _ = t.child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn id_of(started: &str) -> String {
        started.split_whitespace().nth(1).unwrap().to_string()
    }

    fn log_of(started: &str) -> PathBuf {
        PathBuf::from(
            started
                .split("Output: ")
                .nth(1)
                .unwrap()
                .split(" (read")
                .next()
                .unwrap(),
        )
    }

    #[test]
    fn runs_reports_and_stops_tasks() {
        let cwd = std::env::temp_dir();
        let quick = start(&cwd, &json!({"command": "echo built; exit 3"})).unwrap();
        let slow = start(&cwd, &json!({"command": "echo serving; sleep 30"})).unwrap();
        let (quick_id, slow_id) = (id_of(&quick), id_of(&slow));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut notes = Vec::new();
        while notes.is_empty() && Instant::now() < deadline {
            notes = finished()
                .into_iter()
                .filter(|n| n.contains(&quick_id))
                .collect();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("exited with code 3"));
        assert_eq!(std::fs::read_to_string(log_of(&quick)).unwrap(), "built\n");
        assert!(
            finished().iter().all(|n| !n.contains(&quick_id)),
            "reported once"
        );

        let started = Instant::now();
        assert!(
            stop(&json!({"task_id": slow_id}))
                .unwrap()
                .starts_with("stopped")
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            finished().iter().all(|n| !n.contains(&slow_id)),
            "a stopped task is not reported again"
        );
        assert!(stop(&json!({"task_id": "bg999"})).is_err());
        for log in [log_of(&quick), log_of(&slow)] {
            let _ = std::fs::remove_file(log);
        }
    }
}
