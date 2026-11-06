//! Finds what changed between two versions of a document.

use ethos_core::{Change, ChangeKind, Rect, TextRun};
use similar::{DiffOp, TextDiff};
use std::time::{Duration, Instant};

const DIFF_TIMEOUT: Duration = Duration::from_secs(2);

fn normalized(run: &TextRun) -> String {
    run.text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Compares `other` with the open document's `current` lines, as `(page, run)` in page order.
///
/// Lines are the unit because that is how people scan a change list, and it stays fast on
/// long documents. Lines that merely moved between pages count as unchanged.
pub fn compare_lines(
    other: &[(u32, TextRun)],
    current: &[(u32, TextRun)],
) -> Result<Vec<Change>, String> {
    compare_until(other, current, Instant::now() + DIFF_TIMEOUT)
}

fn compare_until(
    other: &[(u32, TextRun)],
    current: &[(u32, TextRun)],
    deadline: Instant,
) -> Result<Vec<Change>, String> {
    let before: Vec<String> = other.iter().map(|(_, run)| normalized(run)).collect();
    let after: Vec<String> = current.iter().map(|(_, run)| normalized(run)).collect();
    let old: Vec<&str> = before.iter().map(String::as_str).collect();
    let new: Vec<&str> = after.iter().map(String::as_str).collect();
    let diff = TextDiff::configure()
        .deadline(deadline)
        .diff_slices(&old, &new);
    if Instant::now() >= deadline {
        return Err("Comparing these documents took too long. Try comparing fewer pages.".into());
    }
    let join = |lines: &[&str]| lines.join(" ");
    let rects = |range: std::ops::Range<usize>| -> Vec<Rect> {
        current[range].iter().map(|(_, run)| run.rect).collect()
    };
    let nearest = |index: usize| {
        current
            .get(index)
            .or_else(|| current.last())
            .map_or(1, |(page, _)| *page)
    };
    let mut changes = Vec::new();
    for operation in diff.ops() {
        match *operation {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => changes.push(Change {
                kind: ChangeKind::Removed,
                page: nearest(new_index),
                before: join(&old[old_index..old_index + old_len]),
                after: String::new(),
                rects: Vec::new(),
            }),
            DiffOp::Insert {
                new_index, new_len, ..
            } => changes.push(Change {
                kind: ChangeKind::Added,
                page: nearest(new_index),
                before: String::new(),
                after: join(&new[new_index..new_index + new_len]),
                rects: rects(new_index..new_index + new_len),
            }),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => changes.push(Change {
                kind: ChangeKind::Changed,
                page: nearest(new_index),
                before: join(&old[old_index..old_index + old_len]),
                after: join(&new[new_index..new_index + new_len]),
                rects: rects(new_index..new_index + new_len),
            }),
        }
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, y: f64) -> TextRun {
        TextRun {
            text: text.into(),
            rect: Rect {
                x: 10.0,
                y,
                width: 100.0,
                height: 12.0,
            },
            direction: 0,
        }
    }

    fn lines(page: u32, texts: &[&str]) -> Vec<(u32, TextRun)> {
        texts.iter().map(|text| (page, run(text, 100.0))).collect()
    }

    #[test]
    fn identical_versions_have_no_changes() {
        assert!(
            compare_lines(&lines(1, &["a", "b"]), &lines(1, &["a", "b"]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn edits_additions_and_removals_are_told_apart() {
        let old = lines(1, &["Same", "The fee is 10", "Dropped clause", "End"]);
        let new = lines(2, &["Same", "The fee is 12", "End", "New clause"]);
        let changes = compare_lines(&old, &new).unwrap();
        let kinds: Vec<_> = changes.iter().map(|change| change.kind).collect();
        assert_eq!(kinds, [ChangeKind::Changed, ChangeKind::Added]);
        assert_eq!(changes[0].before, "The fee is 10 Dropped clause");
        assert_eq!(changes[0].after, "The fee is 12");
        assert_eq!(changes[0].rects.len(), 1);
        assert_eq!(changes[1].page, 2);
    }

    #[test]
    fn removed_text_points_at_the_nearest_remaining_page() {
        let changes = compare_lines(&lines(1, &["Keep", "Remove"]), &lines(3, &["Keep"])).unwrap();
        assert_eq!(changes[0].kind, ChangeKind::Removed);
        assert_eq!(changes[0].page, 3);
        assert!(changes[0].rects.is_empty());
    }

    #[test]
    fn spacing_differences_are_not_changes() {
        assert!(
            compare_lines(&lines(1, &["two  spaces"]), &lines(1, &["two spaces"]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn expired_comparisons_report_an_error_instead_of_approximate_changes() {
        let error =
            compare_until(&lines(1, &["Old"]), &lines(1, &["New"]), Instant::now()).unwrap_err();
        assert!(error.contains("took too long"));
    }
}
