//! A local record of past readings, so a window can be watched burning down.
//!
//! Deliberately not in the GUI: the tray refreshes every few minutes whether or not a
//! window is open, and it is the only surface running often enough to build a useful
//! series. A history that only accumulates while you are looking at it is no history.
//!
//! JSONL rather than a database. One line per reading appends without rewriting, survives
//! a truncated write (a corrupt tail costs one sample), and can be read with any tool.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::config::Config;
use crate::model::ProviderId;
use crate::paths::Env;

/// Readings older than this are dropped when the file is compacted. Long enough to see a
/// monthly window's shape, short enough that the file stays small and readable.
pub const RETENTION_DAYS: i64 = 45;

/// Compaction happens on write, but only when the file has grown past this — rewriting a
/// file on every refresh would be the wrong trade for a few kilobytes.
const COMPACT_ABOVE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reading {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub provider: ProviderId,
    pub label: String,
    pub used_percent: f64,
}

pub fn history_path(env: &Env) -> PathBuf {
    Config::default_path(env)
        .parent()
        .unwrap_or(Path::new("."))
        .join("history.jsonl")
}

/// Appends one reading per window in `results`. Failures are silent by design: a tray that
/// stops reporting quota because a log file is read-only would be a poor trade.
pub fn record(env: &Env, results: &crate::Results) {
    let path = history_path(env);
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }

    let now = OffsetDateTime::now_utc();
    let mut lines = String::new();
    for (provider, outcome) in results {
        let Ok(snapshot) = outcome else { continue };
        for window in &snapshot.windows {
            let reading = Reading {
                at: now,
                provider: *provider,
                label: window.label.clone(),
                used_percent: window.used_percent,
            };
            if let Ok(line) = serde_json::to_string(&reading) {
                lines.push_str(&line);
                lines.push('\n');
            }
        }
    }
    if lines.is_empty() {
        return;
    }

    if let Ok(metadata) = std::fs::metadata(&path)
        && metadata.len() > COMPACT_ABOVE_BYTES
    {
        compact(&path, now);
    }

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(lines.as_bytes());
    }
}

/// Every reading on file, oldest first. Unparseable lines are skipped rather than fatal —
/// a partial write at the tail must not lose the whole series.
pub fn load(env: &Env) -> Vec<Reading> {
    let Ok(raw) = std::fs::read_to_string(history_path(env)) else {
        return Vec::new();
    };
    let mut readings: Vec<Reading> = raw
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    readings.sort_by_key(|reading| reading.at);
    readings
}

/// The series for one window, oldest first.
pub fn series(
    readings: &[Reading],
    provider: ProviderId,
    label: &str,
) -> Vec<(OffsetDateTime, f64)> {
    readings
        .iter()
        .filter(|reading| reading.provider == provider && reading.label == label)
        .map(|reading| (reading.at, reading.used_percent))
        .collect()
}

fn compact(path: &Path, now: OffsetDateTime) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let cutoff = now - time::Duration::days(RETENTION_DAYS);
    let kept: String = raw
        .lines()
        .filter(|line| match serde_json::from_str::<Reading>(line) {
            Ok(reading) => reading.at >= cutoff,
            // An unparseable line is dropped here rather than kept forever: compaction is
            // the only place the file ever shrinks.
            Err(_) => false,
        })
        .fold(String::new(), |mut acc, line| {
            acc.push_str(line);
            acc.push('\n');
            acc
        });
    let _ = std::fs::write(path, kept);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RateWindow, UsageSnapshot};

    fn env_at(dir: &Path) -> Env {
        [(
            "AXIO_QUOTA_CONFIG".to_string(),
            dir.join("config.json").display().to_string(),
        )]
        .into_iter()
        .collect()
    }

    #[test]
    fn readings_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("axio-quota-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let env = env_at(&dir);
        let _ = std::fs::remove_file(history_path(&env));

        let mut snapshot = UsageSnapshot::new(ProviderId::Claude);
        snapshot.windows = vec![RateWindow::new("5h", 12.0), RateWindow::new("Weekly", 40.0)];
        record(&env, &vec![(ProviderId::Claude, Ok(snapshot))]);

        let readings = load(&env);
        assert_eq!(readings.len(), 2);
        let weekly = series(&readings, ProviderId::Claude, "Weekly");
        assert_eq!(weekly.len(), 1);
        assert_eq!(weekly[0].1, 40.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_line_costs_one_sample_not_the_series() {
        let dir = std::env::temp_dir().join(format!("axio-quota-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let env = env_at(&dir);
        let path = history_path(&env);
        std::fs::write(
            &path,
            "{\"at\":\"2026-08-01T00:00:00Z\",\"provider\":\"codex\",\"label\":\"Weekly\",\"used_percent\":5.0}\n\
             {truncated\n",
        )
        .expect("write");

        assert_eq!(load(&env).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
