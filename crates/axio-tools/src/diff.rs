//! Unified diffs, for previewing a change before it is approved.

use std::path::PathBuf;

use axio_core::protocol::Preview;
use similar::{ChangeTag, TextDiff};

/// Build the preview a human sees before allowing a write.
///
/// Truncated deliberately: an approval prompt showing four thousand lines is
/// not a prompt anyone reads, and the tail of a huge diff is where a reviewer
/// stops looking anyway.
const MAX_DIFF_LINES: usize = 200;

/// Unchanged lines kept either side of a change, as in `diff -U3`.
const CONTEXT: usize = 3;

pub fn unified(path: &str, before: &str, after: &str) -> Preview {
    let diff = TextDiff::from_lines(before, after);
    let mut added = 0u32;
    let mut removed = 0u32;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }

    // Hunks, not a walk of the whole file. Truncating a full walk head-first
    // spends the whole budget on unchanged context and drops the change itself
    // out of the preview for any edit past line MAX_DIFF_LINES — an approval
    // prompt that shows a reviewer everything except what they are approving.
    let mut lines = Vec::new();
    let mut elided = 0usize;
    for group in diff.grouped_ops(CONTEXT) {
        let Some(first) = group.first() else { continue };
        let (old_start, old_len, new_start, new_len) = group.iter().fold(
            (
                first.old_range().start,
                0usize,
                first.new_range().start,
                0usize,
            ),
            |(os, ol, ns, nl), op| (os, ol + op.old_range().len(), ns, nl + op.new_range().len()),
        );

        if lines.len() < MAX_DIFF_LINES {
            lines.push(format!(
                "@@ -{},{} +{},{} @@",
                old_start + 1,
                old_len,
                new_start + 1,
                new_len
            ));
        }

        for op in group {
            for change in diff.iter_changes(&op) {
                let sign = match change.tag() {
                    ChangeTag::Insert => "+",
                    ChangeTag::Delete => "-",
                    ChangeTag::Equal => " ",
                };
                if lines.len() < MAX_DIFF_LINES {
                    lines.push(format!("{sign}{}", change.value().trim_end_matches('\n')));
                } else {
                    elided += 1;
                }
            }
        }
    }

    if elided > 0 {
        lines.push(format!("… {elided} more line(s)"));
    }

    Preview::Diff {
        path: PathBuf::from(path),
        unified: lines.join("\n"),
        added,
        removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(p: &Preview) -> (u32, u32, String) {
        match p {
            Preview::Diff {
                added,
                removed,
                unified,
                ..
            } => (*added, *removed, unified.clone()),
            other => panic!("expected a diff, got {other:?}"),
        }
    }

    #[test]
    fn a_new_file_is_all_additions() {
        let (added, removed, text) = counts(&unified("a.rs", "", "one\ntwo\n"));
        assert_eq!((added, removed), (2, 0));
        assert!(text.contains("+one"));
    }

    #[test]
    fn a_replacement_counts_both_sides() {
        let (added, removed, text) = counts(&unified("a.rs", "one\ntwo\n", "one\nTWO\n"));
        assert_eq!((added, removed), (1, 1));
        assert!(text.contains("-two"));
        assert!(text.contains("+TWO"));
        assert!(text.contains(" one"), "context should be shown");
    }

    #[test]
    fn an_enormous_diff_is_truncated_rather_than_unreadable() {
        let after = (0..5_000)
            .map(|i| format!("line {i}\n"))
            .collect::<String>();
        let (added, _, text) = counts(&unified("big.rs", "", &after));
        assert_eq!(added, 5_000, "the count is of the whole change");
        assert!(
            text.lines().count() <= MAX_DIFF_LINES + 1,
            "the shown diff must stay reviewable"
        );
        assert!(text.contains("more line(s)"));
    }

    #[test]
    fn a_change_past_the_line_budget_is_still_in_the_preview() {
        // The whole point of the preview is that the approver sees the change.
        // A whole-file walk truncated head-first shows 200 lines of untouched
        // context and elides the one line that matters.
        let before: String = (0..300).map(|i| format!("VALUE_{i} = {i}\n")).collect();
        let after = before.replace("VALUE_290 = 290", "VALUE_290 = 999");
        let (added, removed, text) = counts(&unified("big.py", &before, &after));
        assert_eq!((added, removed), (1, 1));
        assert!(text.contains("-VALUE_290 = 290"), "got:\n{text}");
        assert!(text.contains("+VALUE_290 = 999"), "got:\n{text}");
        assert!(
            !text.contains("VALUE_0 = 0"),
            "context far from the change does not belong in the preview:\n{text}"
        );
    }

    #[test]
    fn a_hunk_header_locates_the_change_in_the_file() {
        let before: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let after = before.replace("line 40", "LINE 40");
        let (_, _, text) = counts(&unified("a.rs", &before, &after));
        assert!(text.contains("@@ -"), "got:\n{text}");
    }

    #[test]
    fn many_scattered_changes_still_stop_at_the_line_budget() {
        let before: String = (0..5_000).map(|i| format!("line {i}\n")).collect();
        let after: String = (0..5_000).map(|i| format!("LINE {i}\n")).collect();
        let (added, _, text) = counts(&unified("big.rs", &before, &after));
        assert_eq!(added, 5_000, "the count is of the whole change");
        assert!(text.lines().count() <= MAX_DIFF_LINES + 2);
        assert!(text.contains("more line(s)"));
    }

    #[test]
    fn no_change_is_an_empty_diff() {
        let (added, removed, _) = counts(&unified("a.rs", "same\n", "same\n"));
        assert_eq!((added, removed), (0, 0));
    }
}
