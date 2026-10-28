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
    fn borrowing_truncation_matches_the_owned_one() {
        for text in ["", "hello", "città 🚀", "a\u{301}b"] {
            for max in 0..12 {
                assert_eq!(truncate_to_chars(text, max), truncate_chars(text, max));
            }
        }
    }
}
