//! Where sessions live on disk, and how one is found again.
//!
//! The path is derived from the id's own timestamp, so resuming never scans a
//! directory, and every path is built from the re-encoded id.

use super::*;

/// Where sessions live on disk.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file for a session.
    ///
    /// The directory comes from the id's own timestamp, so resuming never has
    /// to scan, and the file name is the re-encoded id — a ULID cannot contain
    /// a path separator or `..`, so a hostile `--resume` argument dies at
    /// parsing rather than at `join`.
    pub fn path_for(&self, id: SessionId) -> PathBuf {
        self.root.join(day_of(id)).join(format!("{id}.jsonl"))
    }

    /// Every session file, newest first.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(days) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for day in days.filter_map(Result::ok) {
            let Ok(entries) = std::fs::read_dir(day.path()) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "jsonl") {
                    out.push(path);
                }
            }
        }
        // The file name is the ULID, which sorts lexicographically by time.
        out.sort();
        out.reverse();
        out
    }

    /// Resolve a full or partial id.
    ///
    /// A 26-character identifier typed by hand is a usability failure, and
    /// resolving a prefix needs only the file names — no header is read.
    pub fn resolve(&self, needle: &str) -> Result<SessionId, String> {
        let typed = needle.trim();
        let needle = typed.to_ascii_uppercase();
        if needle.is_empty() {
            return Err("no session id given".into());
        }
        // A well-formed id still has to exist. Returning it unchecked turns a
        // mistyped or wrong-`AXIO_STATE` id — the likeliest miss, since a full
        // id is what `--list` hands you to copy — into a bare "No such file or
        // directory (os error 2)" from the loader, which reads as a fault in
        // axio rather than an answer about the session.
        if let Ok(id) = needle.parse::<SessionId>()
            && self.path_for(id).is_file()
        {
            return Ok(id);
        }

        let matches: Vec<SessionId> = self
            .files()
            .iter()
            .filter_map(|p| p.file_stem()?.to_str()?.parse::<SessionId>().ok())
            .filter(|id| id.to_string().starts_with(&needle))
            .collect();

        // Quoted as typed: echoing the uppercased needle back tells the user
        // they typed something they did not.
        match matches.len() {
            0 => Err(format!("no session matches `{typed}`")),
            1 => Ok(matches[0]),
            n => Err(format!(
                "`{typed}` matches {n} sessions; give more characters"
            )),
        }
    }
}

/// The `yyyy-mm-dd` a session id was minted on, from the id itself.
fn day_of(id: SessionId) -> String {
    let ms = id.timestamp_ms();
    let days = (ms / 86_400_000) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since the epoch to a calendar date.
///
/// Hand-rolled rather than pulling in a date library for one call: the core
/// crate's dependency count is a property worth keeping.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Read only the header of a session file.
///
/// One line, one deserialisation, a bounded read. This is what makes `--list`
/// cheap enough to stay useful as sessions accumulate.
pub fn read_header(path: &Path) -> std::io::Result<Header> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file.take(HEADER_MAX_BYTES));
    let mut line = String::new();
    reader.read_line(&mut line)?;

    match serde_json::from_str::<Record>(line.trim()) {
        Ok(Record::Header { header }) => Ok(header),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no header on the first line",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_is_derived_correctly_from_a_ulid() {
        // A known ULID timestamp, so a broken calendar conversion cannot hide.
        let id: SessionId = "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap();
        assert_eq!(day_of(id), "2016-07-30");
    }
}
