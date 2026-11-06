use crate::SearchHit;

const BEFORE: usize = 35;
const AFTER: usize = 85;

/// Finds every case-insensitive, non-overlapping `query` in `text` for one page.
pub fn find_hit(page: u32, text: &str, query: &str) -> Option<SearchHit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    let first = lower.find(&needle)?;
    let count = lower.matches(&needle).count();
    // Lowercasing can change byte lengths, so the snippet is cut by characters.
    let start = lower[..first].chars().count();
    let span = needle.chars().count();
    let snippet: String = text
        .chars()
        .skip(start.saturating_sub(BEFORE))
        .take(start.min(BEFORE) + span + AFTER)
        .collect();
    Some(SearchHit {
        page,
        count,
        snippet: format!("{}{snippet}…", if start > BEFORE { "…" } else { "" }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_every_case_insensitive_match() {
        let hit = find_hit(1, "Attention attention and reading", " attention ").unwrap();
        assert_eq!((hit.page, hit.count), (1, 2));
        assert_eq!(hit.snippet, "Attention attention and reading…");
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find_hit(1, "aaaa", "aa").unwrap().count, 2);
    }

    #[test]
    fn blank_queries_and_absent_text_find_nothing() {
        assert_eq!(find_hit(1, "reading", "   "), None);
        assert_eq!(find_hit(1, "reading", "writing"), None);
    }

    #[test]
    fn long_text_keeps_a_window_around_the_match() {
        let text = format!("{}needle{}", "a".repeat(100), "b".repeat(200));
        let hit = find_hit(1, &text, "needle").unwrap();
        assert_eq!(
            hit.snippet,
            format!("…{}needle{}…", "a".repeat(35), "b".repeat(85))
        );
    }

    #[test]
    fn snippets_cut_on_character_boundaries() {
        let text = format!("{}é needle é", "é".repeat(80));
        assert!(
            find_hit(1, &text, "NEEDLE")
                .unwrap()
                .snippet
                .contains("needle")
        );
    }
}
