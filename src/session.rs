use crate::compact;
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
        let err = |e: std::io::Error| format!("{}: {e}", path.display());
        // A crash can leave a half-written last line; start ours on a fresh one.
        let torn = last_byte(&path).is_some_and(|c| c != b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(err)?;
        if torn {
            file.write_all(b"\n").map_err(err)?;
        }
        Ok(Session { path, file })
    }

    pub fn append(&mut self, m: &Message) -> Result<(), String> {
        self.write_line(&json!({"type": "message", "message": m}))
    }

    /// The conversation goes back to its first `keep` messages.
    pub fn rewind(&mut self, keep: usize) -> Result<(), String> {
        self.write_line(&json!({"type": "rewind", "keep": keep}))
    }

    pub fn compaction(&mut self, summary: &str) -> Result<(), String> {
        self.write_line(&json!({"type": "compaction", "summary": summary}))
    }

    fn write_line(&mut self, v: &Value) -> Result<(), String> {
        writeln!(self.file, "{v}").map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// Loads a transcript. Compaction entries replay the same summary the live
/// session applied, and a tool call left without a result by an interrupted
/// run is closed wherever it sits, so the provider accepts the history.
pub fn load(path: &Path) -> Result<Vec<Message>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut messages = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue; // a line cut short by a crash
        };
        match v["type"].as_str() {
            Some("message") => {
                if let Ok(m) = serde_json::from_value::<Message>(v["message"].clone()) {
                    close_open_calls(&mut messages, Some(&m));
                    compact::append(&mut messages, m);
                }
            }
            Some("compaction") => {
                if let Some(summary) = v["summary"].as_str() {
                    close_open_calls(&mut messages, None);
                    compact::apply(&mut messages, summary);
                }
            }
            Some("rewind") => {
                if let Some(keep) = v["keep"].as_u64() {
                    messages.truncate(keep as usize);
                }
            }
            _ => {}
        }
    }
    close_open_calls(&mut messages, None);
    Ok(messages)
}

/// Adds error results for calls in the last message that `next` does not answer.
fn close_open_calls(messages: &mut Vec<Message>, next: Option<&Message>) {
    let Some(last) = messages.last() else { return };
    let answered: Vec<&str> = match next {
        Some(Message::User { content }) => content
            .iter()
            .filter_map(|b| match b {
                Block::ToolResult { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let open: Vec<Block> = last
        .tool_calls()
        .into_iter()
        .filter(|(id, _, _)| !answered.contains(id))
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

/// The session in `dir` used most recently.
pub fn latest(dir: &Path) -> Option<PathBuf> {
    by_last_use(dir).into_iter().next()
}

/// Session files in `dir`, most recently used first, without reading them.
fn by_last_use(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by(|a, b| b.cmp(a));
    files.into_iter().map(|(_, p)| p).collect()
}

fn last_byte(path: &Path) -> Option<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::End(-1)).ok()?;
    let mut b = [0u8];
    f.read_exact(&mut b).ok()?;
    Some(b[0])
}

/// A saved session, for choosing one to continue.
pub struct Summary {
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
    pub first_prompt: String,
    pub messages: usize,
}

/// The sessions in `dir`, most recently used first.
pub fn recent(dir: &Path, limit: usize) -> Vec<Summary> {
    // Only the newest few are read, however many sessions there are.
    by_last_use(dir)
        .into_iter()
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            let messages = load(&path).ok()?;
            let first_prompt = messages
                .iter()
                .find_map(|m| match m {
                    Message::User { content } => content.iter().find_map(|b| match b {
                        Block::Text { text } if !text.starts_with('[') => Some(text.clone()),
                        _ => None,
                    }),
                    Message::Assistant { .. } => None,
                })
                .unwrap_or_default();
            Some(Summary {
                path,
                modified,
                first_prompt,
                messages: messages.len(),
            })
        })
        .filter(|s| s.messages > 0)
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::message::Message as M;

    #[test]
    fn a_rewind_line_replays_on_load() {
        let dir = std::env::temp_dir().join(format!("axio-rewind-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::create(&dir, &dir).unwrap();
        for m in [
            M::user_text("one"),
            M::Assistant {
                content: vec![crate::message::Block::Text { text: "a".into() }],
            },
            M::user_text("two"),
        ] {
            s.append(&m).unwrap();
        }
        s.rewind(2).unwrap();
        s.append(&M::user_text("three")).unwrap();
        let loaded = load(&s.path).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[2].text(), "three");
    }
    use super::*;

    #[test]
    fn latest_is_the_one_used_last() {
        let dir = std::env::temp_dir().join(format!("axio-latest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (old, new) = (dir.join("100-1.jsonl"), dir.join("200-1.jsonl"));
        std::fs::write(&new, "{}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&old, "{}\n").unwrap();
        assert_eq!(
            latest(&dir),
            Some(old),
            "used after the newer-named one was created"
        );
        assert_eq!(last_byte(&new), Some(b'\n'));
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    fn call(id: &str) -> Message {
        Message::Assistant {
            content: vec![Block::ToolCall {
                id: id.into(),
                name: "bash".into(),
                input: json!({"command": "ls"}),
            }],
        }
    }

    #[test]
    fn closes_calls_left_open_mid_history_and_replays_compaction() {
        let dir = std::env::temp_dir().join(format!("axio-session-mid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::create(&dir, Path::new("/w")).unwrap();
        s.append(&Message::user_text("one")).unwrap();
        s.append(&call("t1")).unwrap();
        let path = s.path.clone();
        drop(s);
        // A crash mid-line, then a resumed run carries on in the same file.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"ty")
            .unwrap();
        let mut s = Session::open(path.clone()).unwrap();
        s.append(&Message::user_text("two")).unwrap();
        s.append(&Message::Assistant {
            content: vec![Block::Text { text: "ok".into() }],
        })
        .unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.len(), 4);
        let Message::User { content } = &loaded[2] else {
            panic!()
        };
        assert!(matches!(&content[0], Block::ToolResult { id, is_error: true, .. } if id == "t1"));
        assert_eq!(content[1], Block::Text { text: "two".into() });

        s.compaction("did one").unwrap();
        let compacted = load(&path).unwrap();
        assert_eq!(compacted.len(), 1);
        let Message::User { content } = &compacted[0] else {
            panic!()
        };
        assert!(content.iter().all(|b| matches!(b, Block::Text { .. })));
        assert!(compacted[0].text().contains("two") && compacted[0].text().contains("did one"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
