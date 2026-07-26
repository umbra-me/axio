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

pub fn unified(path: &str, before: &str, after: &str) -> Preview {
    let diff = TextDiff::from_lines(before, after);
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut lines = Vec::new();
    let mut shown = 0usize;
    let mut elided = 0usize;

    for change in diff.iter_all_changes() {
        let (sign, counts) = match change.tag() {
            ChangeTag::Insert => ("+", Some(&mut added)),
            ChangeTag::Delete => ("-", Some(&mut removed)),
            ChangeTag::Equal => (" ", None),
        };
        if let Some(c) = counts {
            *c += 1;
        }
        if shown < MAX_DIFF_LINES {
            lines.push(format!("{sign}{}", change.value().trim_end_matches('\n')));
            shown += 1;
        } else {
            elided += 1;
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
    fn no_change_is_an_empty_diff() {
        let (added, removed, _) = counts(&unified("a.rs", "same\n", "same\n"));
        assert_eq!((added, removed), (0, 0));
    }
}
