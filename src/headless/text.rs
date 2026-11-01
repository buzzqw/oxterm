//! Small string helpers shared by the headless core and the GUI.

/// Truncate to at most `max` characters, never splitting a UTF-8 sequence.
pub fn truncate_chars(text: &str, max: usize) -> String {
    truncate_to_chars(text, max).to_string()
}

/// Borrowing form of [`truncate_chars`], for callers that only need a slice.
pub fn truncate_to_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

/// Score a fuzzy subsequence match, higher is better.
///
/// The command palette uses this for both actions and `/commands`: a query
/// matches when all of its non-space characters appear in order, with bonuses
/// for consecutive characters and word starts. `None` means no match.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut matched = 0;
    let mut last: Option<usize> = None;
    for (index, c) in text.iter().enumerate() {
        if matched >= query.len() || *c != query[matched] {
            continue;
        }
        score += 1;
        if last == Some(index.saturating_sub(1)) {
            score += 3;
        }
        if index == 0 || !text[index - 1].is_alphanumeric() {
            score += 2;
        }
        last = Some(index);
        matched += 1;
    }
    if matched != query.len() {
        return None;
    }
    let query: String = query.iter().collect();
    let text: String = text.iter().collect();
    if text.starts_with(&query) {
        score += 20;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate_chars("hello", 3), "hel");
        assert_eq!(truncate_chars("hello", 99), "hello");
        assert_eq!(truncate_chars("città", 4), "citt");
        assert_eq!(truncate_chars("città", 5), "città");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn fuzzy_matching_rewards_ordered_prefixes() {
        assert_eq!(fuzzy_score("", "Split vertical"), Some(0));
        assert!(fuzzy_score("spl", "Split vertical").is_some());
        assert!(fuzzy_score("split vert", "Split vertical").is_some());
        assert!(fuzzy_score("vert spl", "Split vertical").is_none());
        assert!(fuzzy_score("zzz", "Split vertical").is_none());
        assert!(
            fuzzy_score("spl", "Split vertical").unwrap()
                > fuzzy_score("spl", "Search split scrollback").unwrap()
        );
    }

    #[test]
    fn borrowing_truncation_matches_the_owned_one() {
        for text in ["", "hello", "città 🚀", "a\u{301}b"] {
            for max in 0..12 {
                assert_eq!(truncate_to_chars(text, max), truncate_chars(text, max));
            }
        }
    }
}
