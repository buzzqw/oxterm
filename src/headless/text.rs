//! Small string helpers shared by the headless core and the GUI.

/// Truncate to at most `max` characters, never splitting a UTF-8 sequence.
pub fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((index, _)) => text[..index].to_string(),
        None => text.to_string(),
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
}
