use std::fmt::Write;

use crate::{Edits, MarkKind};

/// Highlights and notes as Markdown, in page order, ready for a notes app.
pub fn notes_markdown(title: &str, edits: &Edits) -> String {
    let mut output = format!("# {title}\n");
    for (index, entry) in edits.pages.iter().enumerate() {
        let mut lines = String::new();
        for mark in edits.marks.iter().filter(|mark| mark.page == entry.source) {
            match &mark.kind {
                MarkKind::Highlight { text, .. } => {
                    let quote = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    let _ = writeln!(lines, "> {quote}\n");
                }
                MarkKind::Text { text, .. } => {
                    let note = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    let _ = writeln!(lines, "{note}\n");
                }
                MarkKind::Pen { .. } | MarkKind::Signature { .. } | MarkKind::Redaction { .. } => {}
            }
        }
        if !lines.is_empty() {
            let _ = writeln!(output, "\n## Page {}\n\n{}", index + 1, lines.trim_end());
        }
    }
    output.trim_end().to_owned() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InkColor, Mark, PageEntry, Point};

    fn mark(page: u32, kind: MarkKind) -> Mark {
        Mark {
            id: page.to_string(),
            page,
            color: InkColor::Yellow,
            kind,
        }
    }

    fn page(source: u32) -> PageEntry {
        PageEntry {
            source,
            rotation: None,
        }
    }

    #[test]
    fn notes_follow_page_order_and_use_display_numbers() {
        let edits = Edits {
            marks: vec![
                mark(
                    1,
                    MarkKind::Highlight {
                        rects: vec![],
                        text: "A  quoted\nline".into(),
                    },
                ),
                mark(
                    3,
                    MarkKind::Text {
                        point: Point::default(),
                        text: "My\nthought".into(),
                    },
                ),
                mark(2, MarkKind::Pen { points: vec![] }),
            ],
            pages: vec![page(3), page(2), page(1)],
            fields: vec![],
        };
        assert_eq!(
            notes_markdown("Report", &edits),
            "# Report\n\n## Page 1\n\nMy thought\n\n## Page 3\n\n> A quoted line\n"
        );
    }

    #[test]
    fn deleted_pages_leave_their_notes_out() {
        let edits = Edits {
            marks: vec![mark(
                2,
                MarkKind::Text {
                    point: Point::default(),
                    text: "Gone".into(),
                },
            )],
            pages: vec![page(1)],
            fields: vec![],
        };
        assert_eq!(notes_markdown("Report", &edits), "# Report\n");
    }
}
