//! Pure VI copy-mode helpers: word motions, text objects and slicing.
//!
//! These are deliberately independent of VTE and GTK so they can be tested
//! without a display; `terminal/vi_copy.rs` owns the cursor, overlay and
//! clipboard side of the mode.
//!
//! Every helper is called from the key handler, so they work directly on the
//! line's chars: a `Vec<char>` per keystroke costs an allocation and a second
//! pass over the row even though the motions themselves are linear.

pub fn is_word_char(c: char, big: bool) -> bool {
    if big {
        !c.is_whitespace()
    } else {
        c.is_alphanumeric() || c == '_'
    }
}

/// Byte offset of the character at `index`, if the line is long enough.
fn char_offset(line: &str, index: usize) -> Option<usize> {
    line.char_indices().nth(index).map(|(offset, _)| offset)
}

/// Character count and `col` clamped to the last character, or `None` when the
/// line is empty.
fn clamp_col(line: &str, col: usize) -> Option<(usize, usize)> {
    let count = line.chars().count();
    (count > 0).then(|| (count, col.min(count - 1)))
}

/// Start of the next word after `col` (as Vim's `w`), if it is on this line.
pub fn next_word_start(line: &str, col: usize, big: bool) -> Option<usize> {
    let mut index = col;
    let mut chars = line.chars().skip(col).peekable();
    if chars.peek().is_some_and(|c| is_word_char(*c, big)) {
        while chars.peek().is_some_and(|c| is_word_char(*c, big)) {
            chars.next();
            index += 1;
        }
    }
    while chars.peek().is_some_and(|c| !is_word_char(*c, big)) {
        chars.next();
        index += 1;
    }
    chars.peek().is_some().then_some(index)
}

/// Start of the word before `col` (as Vim's `b`), if it is on this line.
pub fn prev_word_start(line: &str, col: usize, big: bool) -> Option<usize> {
    if line.is_empty() {
        return None;
    }
    let count = line.chars().count();
    let mut index = col.min(count - 1);
    let mut offset = char_offset(line, index)?;
    let current = |offset: usize| line[offset..].chars().next().unwrap();
    let previous = |offset: usize| line[..offset].chars().next_back().unwrap();
    // Vim's `b` from the first character of a word goes to the previous word,
    // not to the current one.
    if index > 0 && is_word_char(current(offset), big) && !is_word_char(previous(offset), big) {
        index -= 1;
        offset -= previous(offset).len_utf8();
    }
    if index > 0 && !is_word_char(current(offset), big) && is_word_char(previous(offset), big) {
        index -= 1;
        offset -= previous(offset).len_utf8();
    }
    while index > 0 && !is_word_char(current(offset), big) {
        index -= 1;
        offset -= previous(offset).len_utf8();
    }
    while index > 0 && is_word_char(previous(offset), big) {
        index -= 1;
        offset -= previous(offset).len_utf8();
    }
    Some(index)
}

/// End of the current or next word (as Vim's `e`), if it is on this line.
pub fn word_end(line: &str, col: usize, big: bool) -> Option<usize> {
    let (_, mut index) = clamp_col(line, col)?;
    let mut chars = line.chars().skip(index).peekable();
    while chars.peek().is_some_and(|c| !is_word_char(*c, big)) {
        chars.next();
        index += 1;
    }
    chars.peek()?;
    chars.next();
    while chars.peek().is_some_and(|c| is_word_char(*c, big)) {
        chars.next();
        index += 1;
    }
    Some(index)
}

fn quote_pair(line: &str, col: usize, quote: char) -> Option<(usize, usize)> {
    let mut open: Option<usize> = None;
    let mut first: Option<(usize, usize)> = None;
    for (index, c) in line.chars().enumerate() {
        if c != quote {
            continue;
        }
        match open.take() {
            None => open = Some(index),
            Some(start) => {
                if start <= col && col <= index {
                    return Some((start, index));
                }
                first.get_or_insert((start, index));
            }
        }
    }
    first
}

fn bracket_pair(line: &str, col: usize, open: char, close: char) -> Option<(usize, usize)> {
    let count = line.chars().count();
    if count == 0 {
        return None;
    }
    let col = col.min(count - 1);

    // Innermost pair that contains `col`: walk backwards from the cursor,
    // cancelling pairs that close before it, then walk forward to the closing
    // bracket of the first opening one that is still unmatched.
    //
    // A close at the cursor itself belongs to the pair that contains it, so it
    // starts the depth at one instead of being cancelled.
    let starts_on_close = line.chars().nth(col) == Some(close);
    let mut depth = usize::from(starts_on_close);
    let mut open_at = None;
    let mut index = col;
    let mut skip_first = starts_on_close;
    for c in line.chars().rev().skip(count - 1 - col) {
        if skip_first {
            skip_first = false;
        } else if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                open_at = Some(index);
                break;
            }
            depth -= 1;
            if starts_on_close && depth == 0 {
                open_at = Some(index);
                break;
            }
        }
        if index == 0 {
            break;
        }
        index -= 1;
    }
    if let Some(start) = open_at {
        let mut depth = 0usize;
        for (index, c) in line.chars().enumerate().skip(start) {
            if c == open {
                depth += 1;
            } else if c == close {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((start, index));
                }
            }
        }
    }

    // Otherwise the first pair that starts at or after the cursor, which is
    // the first close that still has an opening bracket since `col` on the
    // (implicit) stack.
    let mut depth = 0usize;
    let mut last_open = None;
    for (index, c) in line.chars().enumerate().skip(col) {
        if c == open {
            depth += 1;
            last_open = Some(index);
        } else if c == close && depth > 0 {
            return last_open.map(|start| (start, index));
        }
    }
    None
}

fn word_object(line: &str, col: usize, big: bool, around: bool) -> Option<(usize, usize)> {
    let (_, mut index) = clamp_col(line, col)?;
    let mut chars = line.chars().skip(index).peekable();
    while chars.peek().is_some_and(|c| !is_word_char(*c, big)) {
        chars.next();
        index += 1;
    }
    chars.peek()?;
    chars.next();
    let mut end = index;
    while chars.peek().is_some_and(|c| is_word_char(*c, big)) {
        chars.next();
        end += 1;
    }

    let mut start = index;
    let mut offset = char_offset(line, start)?;
    while start > 0 {
        let previous = line[..offset].chars().next_back()?;
        if !is_word_char(previous, big) {
            break;
        }
        offset -= previous.len_utf8();
        start -= 1;
    }

    if around {
        match char_offset(line, end + 1) {
            Some(after) if line[after..].chars().next().unwrap().is_whitespace() => {
                let mut last = end + 1;
                let mut next = after;
                while let Some(c) = line[next..].chars().next() {
                    if !c.is_whitespace() {
                        break;
                    }
                    next += c.len_utf8();
                    last += 1;
                }
                end = last - 1;
            }
            _ if start > 0
                && line[..offset]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace) =>
            {
                let mut first = start;
                let mut next = offset;
                while first > 0 {
                    let previous = line[..next].chars().next_back().unwrap();
                    if !previous.is_whitespace() {
                        break;
                    }
                    next -= previous.len_utf8();
                    first -= 1;
                }
                start = first;
            }
            _ => {}
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
    let count = line.chars().count();
    if count == 0 {
        return None;
    }
    let col = col.min(count - 1);
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

    fn reference_next_word_start(line: &str, col: usize, big: bool) -> Option<usize> {
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

    fn reference_prev_word_start(line: &str, col: usize, big: bool) -> Option<usize> {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let mut i = col.min(chars.len() - 1);
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

    fn reference_word_end(line: &str, col: usize, big: bool) -> Option<usize> {
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

    #[test]
    fn word_motions_match_the_vec_scan() {
        let lines = [
            "let mut value = foo(bar);",
            "città è bella",
            "  leading and trailing  ",
            "foo(bar) baz",
            "...",
            "one",
            "",
            "a_b-c.d",
        ];
        for line in lines {
            let count = line.chars().count();
            for col in 0..=count + 3 {
                for big in [false, true] {
                    assert_eq!(
                        next_word_start(line, col, big),
                        reference_next_word_start(line, col, big),
                        "next line={line:?} col={col} big={big}"
                    );
                    assert_eq!(
                        prev_word_start(line, col, big),
                        reference_prev_word_start(line, col, big),
                        "prev line={line:?} col={col} big={big}"
                    );
                    assert_eq!(
                        word_end(line, col, big),
                        reference_word_end(line, col, big),
                        "end line={line:?} col={col} big={big}"
                    );
                }
            }
        }
    }

    /// The previous, `Vec<char>`-based bracket scan, kept here as an oracle.
    fn reference_bracket(
        line: &str,
        col: usize,
        open: char,
        close: char,
    ) -> Option<(usize, usize)> {
        let chars: Vec<char> = line.chars().collect();
        let mut stack = Vec::new();
        let mut best = None;
        let mut first_after = None;
        for (index, &c) in chars.iter().enumerate() {
            if c == open {
                stack.push(index);
            } else if c == close {
                if let Some(start) = stack.pop() {
                    if start <= col && col <= index {
                        if best.is_none_or(|(bo, bi)| start >= bo && index <= bi) {
                            best = Some((start, index));
                        }
                    } else if start >= col && first_after.is_none() {
                        first_after = Some((start, index));
                    }
                }
            }
        }
        best.or(first_after)
    }

    #[test]
    fn bracket_objects_match_the_stack_scan() {
        let lines = [
            "f(a, (b), c)",
            "a(b(c)d)e",
            "(a)(b)",
            "a[b[c]d]e",
            "<a><b<c>>",
            "((()))",
            "(()",
            "())",
            "no brackets",
            "(a,b)",
            "x(a)y(b)z",
            "a(b[c]d)e",
            "{a(b)c}",
            "(a (b) c)",
            "a(b)c(d(e)f)g",
        ];
        for line in lines {
            let count = line.chars().count();
            for col in 0..=count + 2 {
                // `text_object_range` clamps the column before calling the
                // bracket scan, so compare against the same clamped position.
                let col = col.min(count - 1);
                for (open, close) in [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')] {
                    assert_eq!(
                        bracket_pair(line, col, open, close),
                        reference_bracket(line, col, open, close),
                        "line={line:?} col={col} brackets={open}{close}"
                    );
                }
            }
        }
    }
}
