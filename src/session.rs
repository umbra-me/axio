use crate::message::{Block, Message};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// An append-only JSONL transcript: a header line, then one line per message.
pub struct Session {
    pub path: PathBuf,
    file: File,
}

impl Session {
    pub fn create(dir: &Path, cwd: &Path) -> Result<Session, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let path = dir.join(format!(
            "{:013}-{}.jsonl",
            now.as_millis(),
            std::process::id()
        ));
        let mut session = Session::open(path)?;
        session.write_line(&json!({
            "type": "session",
            "version": 1,
            "cwd": cwd.display().to_string(),
            "created_ms": now.as_millis() as u64,
        }))?;
        Ok(session)
    }

    pub fn open(path: PathBuf) -> Result<Session, String> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Session { path, file })
    }

    pub fn append(&mut self, m: &Message) -> Result<(), String> {
        self.write_line(&json!({"type": "message", "message": m}))
    }

    fn write_line(&mut self, v: &Value) -> Result<(), String> {
        writeln!(self.file, "{v}").map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// Loads a transcript, closing any tool call left without a result by an
/// interrupted run so the provider accepts the history.
pub fn load(path: &Path) -> Result<Vec<Message>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut messages = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue; // a line cut short by a crash
        };
        if v["type"] == "message"
            && let Ok(m) = serde_json::from_value::<Message>(v["message"].clone())
        {
            messages.push(m);
        }
    }
    if let Some(last) = messages.last() {
        let open: Vec<Block> = last
            .tool_calls()
            .into_iter()
            .map(|(id, _, _)| Block::ToolResult {
                id: id.to_string(),
                output: "interrupted before the tool ran".into(),
                is_error: true,
            })
            .collect();
        if !open.is_empty() {
            messages.push(Message::User { content: open });
        }
    }
    Ok(messages)
}

/// Sessions are grouped by working directory, like `--Users-me-project--`.
pub fn dir_for(home: &Path, cwd: &Path) -> PathBuf {
    let slug: String = cwd
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join("sessions").join(format!("-{slug}-"))
}

pub fn latest(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_repairs_interrupted_tool_calls() {
        let dir = std::env::temp_dir().join(format!("axio-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::create(&dir, Path::new("/w")).unwrap();
        s.append(&Message::user_text("hi")).unwrap();
        s.append(&Message::Assistant {
            content: vec![Block::ToolCall {
                id: "t1".into(),
                name: "bash".into(),
                input: json!({"command": "ls"}),
            }],
        })
        .unwrap();
        let path = s.path.clone();
        drop(s);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"type\":\"mess")
            .unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0], Message::user_text("hi"));
        assert!(matches!(
            &loaded[2],
            Message::User { content } if matches!(&content[0], Block::ToolResult { id, is_error: true, .. } if id == "t1")
        ));
        assert_eq!(latest(&dir), Some(path));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
