//! Pure VI copy-mode helpers: word motions, text objects and slicing.
//!
//! These are deliberately independent of VTE and GTK so they can be tested
//! without a display; `terminal/vi_copy.rs` owns the cursor, overlay and
//! clipboard side of the mode.

pub fn is_word_char(c: char, big: bool) -> bool {
    if big {
        !c.is_whitespace()
    } else {
        c.is_alphanumeric() || c == '_'
    }
}

/// Start of the next word after `col` (as Vim's `w`), if it is on this line.
pub fn next_word_start(line: &str, col: usize, big: bool) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = col.min(chars.len());
    if i < chars.len() && is_word_char(chars[i], big) {
        while i < chars.len() && is_word_char(chars[i], big) {
            i += 1;
        }
    }
    while i < chars.len() && !is_word_char(chars[i], big) {
        i += 1;
    }
    (i < chars.len()).then_some(i)
}

/// Start of the word before `col` (as Vim's `b`), if it is on this line.
pub fn prev_word_start(line: &str, col: usize, big: bool) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut i = col.min(chars.len() - 1);
    // Vim's `b` from the first character of a word goes to the previous word,
    // not to the current one.
    if i > 0 && is_word_char(chars[i], big) && !is_word_char(chars[i - 1], big) {
        i -= 1;
    }
    if i > 0 && !is_word_char(chars[i], big) && is_word_char(chars[i - 1], big) {
        i -= 1;
    }
    while i > 0 && !is_word_char(chars[i], big) {
        i -= 1;
    }
    while i > 0 && is_word_char(chars[i - 1], big) {
        i -= 1;
    }
    Some(i)
}

/// End of the current or next word (as Vim's `e`), if it is on this line.
pub fn word_end(line: &str, col: usize, big: bool) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut i = col.min(chars.len() - 1);
    while i < chars.len() && !is_word_char(chars[i], big) {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    while i + 1 < chars.len() && is_word_char(chars[i + 1], big) {
        i += 1;
    }
    Some(i)
}

fn quote_pair(line: &str, col: usize, quote: char) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut open: Option<usize> = None;
    let mut first: Option<(usize, usize)> = None;
    for (i, &c) in chars.iter().enumerate() {
        if c != quote {
            continue;
        }
        match open.take() {
            None => open = Some(i),
            Some(o) => {
                if o <= col && col <= i {
                    return Some((o, i));
                }
                first.get_or_insert((o, i));
            }
        }
    }
    first
}

fn bracket_pair(line: &str, col: usize, open: char, close: char) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut stack: Vec<usize> = Vec::new();
    let mut best: Option<(usize, usize)> = None;
    let mut first_after: Option<(usize, usize)> = None;
    for (i, &c) in chars.iter().enumerate() {
        if c == open {
            stack.push(i);
        } else if c == close {
            if let Some(o) = stack.pop() {
                if o <= col && col <= i {
                    if best.map_or(true, |(bo, bi)| o >= bo && i <= bi) {
                        best = Some((o, i));
                    }
                } else if o >= col && first_after.is_none() {
                    first_after = Some((o, i));
                }
            }
        }
    }
    best.or(first_after)
}

fn word_object(line: &str, col: usize, big: bool, around: bool) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut i = col.min(chars.len() - 1);
    while i < chars.len() && !is_word_char(chars[i], big) {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    let mut start = i;
    while start > 0 && is_word_char(chars[start - 1], big) {
        start -= 1;
    }
    let mut end = i;
    while end + 1 < chars.len() && is_word_char(chars[end + 1], big) {
        end += 1;
    }
    if around {
        if end + 1 < chars.len() && chars[end + 1].is_whitespace() {
            let mut j = end + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            end = j - 1;
        } else if start > 0 && chars[start - 1].is_whitespace() {
            let mut j = start;
            while j > 0 && chars[j - 1].is_whitespace() {
                j -= 1;
            }
            start = j;
        }
    }
    Some((start, end))
}

/// Inclusive character range of a text object on one line.
///
/// `kind` is one of `w`/`W` (word), `"`/`'`/`` ` `` (quotes), `(`/`[`/`{`/`<`
/// (brackets) or their closing counterparts.
pub fn text_object_range(
    line: &str,
    col: usize,
    kind: char,
    around: bool,
) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let col = col.min(chars.len() - 1);
    match kind {
        'w' => word_object(line, col, false, around),
        'W' => word_object(line, col, true, around),
        '"' | '\'' | '`' => {
            let (open, close) = quote_pair(line, col, kind)?;
            if around {
                Some((open, close))
            } else if close > open + 1 {
                Some((open + 1, close - 1))
            } else {
                Some((open + 1, open + 1))
            }
        }
        '(' | ')' => {
            let (open, close) = bracket_pair(line, col, '(', ')')?;
            Some(if around {
                (open, close)
            } else {
                (open + 1, close.saturating_sub(1))
            })
        }
        '[' | ']' => {
            let (open, close) = bracket_pair(line, col, '[', ']')?;
            Some(if around {
                (open, close)
            } else {
                (open + 1, close.saturating_sub(1))
            })
        }
        '{' | '}' => {
            let (open, close) = bracket_pair(line, col, '{', '}')?;
            Some(if around {
                (open, close)
            } else {
                (open + 1, close.saturating_sub(1))
            })
        }
        '<' | '>' => {
            let (open, close) = bracket_pair(line, col, '<', '>')?;
            Some(if around {
                (open, close)
            } else {
                (open + 1, close.saturating_sub(1))
            })
        }
        _ => None,
    }
}

pub fn chars_slice(line: &str, start: usize, end: usize) -> String {
    line.chars()
        .skip(start)
        .take(end.saturating_sub(start) + 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_motions_follow_vim_semantics() {
        let line = "let mut value = foo(bar);";
        assert_eq!(next_word_start(line, 0, false), Some(4));
        assert_eq!(next_word_start(line, 4, false), Some(8));
        assert_eq!(prev_word_start(line, 8, false), Some(4));
        assert_eq!(prev_word_start(line, 10, false), Some(8));
        assert_eq!(word_end(line, 0, false), Some(2));
        assert_eq!(word_end(line, 4, false), Some(6));
        // Punctuation is skipped by the small-word motions.
        assert_eq!(next_word_start("foo(bar)", 0, false), Some(4));
        assert_eq!(next_word_start("foo(bar)", 3, false), Some(4));
        // BIG words only break on whitespace.
        assert_eq!(next_word_start("foo(bar) baz", 0, true), Some(9));
    }

    #[test]
    fn quote_and_bracket_objects_are_inclusive() {
        assert_eq!(
            text_object_range("say \"hello\" now", 7, '"', false),
            Some((5, 9))
        );
        assert_eq!(
            text_object_range("say \"hello\" now", 7, '"', true),
            Some((4, 10))
        );
        assert_eq!(
            text_object_range("f(a, (b), c)", 7, '(', false),
            Some((6, 6))
        );
        assert_eq!(
            text_object_range("f(a, (b), c)", 7, '(', true),
            Some((5, 7))
        );
        assert_eq!(
            text_object_range("f(a, (b), c)", 0, ')', false),
            Some((6, 6))
        );
    }

    #[test]
    fn word_object_includes_trailing_space_when_around() {
        assert_eq!(
            text_object_range("one two three", 5, 'w', false),
            Some((4, 6))
        );
        assert_eq!(
            text_object_range("one two three", 5, 'w', true),
            Some((4, 7))
        );
    }

    #[test]
    fn chars_slice_is_inclusive_and_safe() {
        assert_eq!(chars_slice("abcdef", 1, 3), "bcd");
        assert_eq!(chars_slice("abc", 2, 99), "c");
        assert_eq!(chars_slice("", 0, 3), "");
    }

    #[test]
    fn motions_never_split_multi_byte_characters() {
        let line = "città è bella";
        let end = word_end(line, 0, false).unwrap();
        assert_eq!(chars_slice(line, 0, end), "città");
        assert_eq!(next_word_start(line, 0, false), Some(6));
    }
}
