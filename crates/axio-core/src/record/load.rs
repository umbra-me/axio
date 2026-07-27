//! Reading a session back, including one that was cut short.
//!
//! A truncated final line is a notice, not an error — the process may simply
//! have been killed mid-write. A hole earlier in the file is different, and
//! marks the load degraded.

use super::*;

/// A session read back from disk.
#[derive(Debug)]
pub struct Loaded {
    pub session: Session,
    pub header: Header,
    pub notices: Vec<Notice>,
    /// A record was lost from the middle of the file, not just the tail. The
    /// transcript has a hole the model would misread as history.
    pub degraded: bool,
}

/// Read a session file back into a transcript.
///
/// Never fails on a damaged line. A truncated final line is a crash, not
/// corruption — erroring there would lose the user's entire session over the
/// last few bytes.
pub fn load(path: &Path) -> std::io::Result<Loaded> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);

    let mut notices = Vec::new();
    let mut header: Option<Header> = None;
    let mut items: Vec<Item> = Vec::new();
    let mut degraded = false;

    let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
    let last = lines.len().saturating_sub(1);

    for (n, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Record = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(e) => {
                if n == last {
                    notices.push(Notice::warn(
                        "the last line of this session was incomplete and was skipped; \
                         it was most likely interrupted mid-write",
                    ));
                } else {
                    notices.push(Notice::error(format!(
                        "line {} of this session could not be read ({e}); it was skipped",
                        n + 1
                    )));
                    degraded = true;
                }
                continue;
            }
        };

        match record {
            Record::Header { header: h } => {
                if h.version > SESSION_FORMAT_VERSION {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "this session was written by a newer axio (format {} > {}); \
                             upgrade to read it",
                            h.version, SESSION_FORMAT_VERSION
                        ),
                    ));
                }
                header = Some(h);
            }
            // Replacing by id is the fold: a tool call is written once when the
            // model emits it and again as its status resolves.
            Record::Item { item } => match items.iter_mut().find(|i| i.id == item.id) {
                Some(existing) => *existing = item,
                None => items.push(item),
            },
            Record::Compacted { .. } | Record::TurnEnded { .. } | Record::Resumed { .. } => {}
            Record::Unknown => {}
        }
    }

    let Some(header) = header else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "this session file has no header line",
        ));
    };

    let mut session =
        Session::from_parts(header.id, header.cwd.clone(), header.model.clone(), items);

    // A `tool_use` with no matching `tool_result` is rejected outright, so a
    // session interrupted mid-call cannot be resumed until every call has an
    // answer. The wording matters: the call may well have completed.
    let orphans = session.unfinished_calls();
    if !orphans.is_empty() {
        for call_id in &orphans {
            session.set_tool_status(call_id, ToolStatus::Cancelled);
        }
        notices.push(Notice::warn(format!(
            "{} tool call(s) had no recorded result and were marked cancelled; \
             they may have completed before the interruption",
            orphans.len()
        )));
    }

    Ok(Loaded {
        session,
        header,
        notices,
        degraded,
    })
}
