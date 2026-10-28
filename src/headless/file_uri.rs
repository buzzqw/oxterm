//! Decoding of the `file://` URIs a shell reports through OSC 7.
//!
//! This is display-only information: Oxterm shows the directory the shell says
//! it is in, but never uses it to record history or to start a command. Those
//! jobs belong to `get_cwd()`, which reads `/proc/<pid>/cwd` and therefore
//! cannot be steered by a program writing escape sequences.
//!
//! The decoder is deliberately strict: only `file://` URIs with no host (or
//! `localhost`) become a path, query and fragment are dropped, control
//! characters are removed and the result is capped, so a hostile or buggy
//! program can only produce an ugly tooltip.

/// Longest path shown to the user, in characters.
pub const MAX_PATH_CHARS: usize = 1024;

/// Turn an OSC 7 URI into a filesystem path to display.
///
/// Returns `None` for anything that is not a local absolute `file://` URI.
pub fn display_path(uri: &str) -> Option<String> {
    let (scheme, rest) = uri.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("file") {
        return None;
    }
    // `file://<host>/<path>`; an empty authority is the common `file:///path`.
    let path = if rest.starts_with('/') {
        rest
    } else {
        let (host, _) = rest.split_once('/')?;
        if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
            return None;
        }
        // Keep the slash the authority was split on.
        &rest[host.len()..]
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = percent_decode(path);
    let path = path.chars().filter(|c| !c.is_control()).collect::<String>();
    if !path.starts_with('/') {
        return None;
    }
    Some(shorten_from_the_start(&path, MAX_PATH_CHARS))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if let Some(byte) = bytes
                .get(index + 1..index + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Keep the tail of a long path: the last component says more than the root.
fn shorten_from_the_start(path: &str, max: usize) -> String {
    let length = path.chars().count();
    if length <= max {
        return path.to_string();
    }
    let keep = max.saturating_sub(1);
    let tail: String = path.chars().skip(length - keep).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_plain_local_uri() {
        assert_eq!(
            display_path("file:///home/ana/src"),
            Some("/home/ana/src".into())
        );
        assert_eq!(display_path("FILE:///tmp"), Some("/tmp".into()));
        assert_eq!(
            display_path("file://localhost/tmp/x"),
            Some("/tmp/x".into())
        );
    }

    #[test]
    fn decodes_escapes_and_spaces() {
        assert_eq!(
            display_path("file:///home/ana/My%20Files"),
            Some("/home/ana/My Files".into())
        );
        // Our own scripts emit the path unencoded, so literal spaces and plus
        // signs must survive untouched.
        assert_eq!(
            display_path("file:///home/ana/My Files/a+b"),
            Some("/home/ana/My Files/a+b".into())
        );
        assert_eq!(display_path("file:///tmp/a%2Fb"), Some("/tmp/a/b".into()));
        // A broken escape stays literal instead of swallowing characters.
        assert_eq!(display_path("file:///tmp/100%"), Some("/tmp/100%".into()));
        assert_eq!(display_path("file:///tmp/%zz"), Some("/tmp/%zz".into()));
    }

    #[test]
    fn rejects_other_hosts_and_schemes() {
        assert_eq!(display_path("file://build-server/tmp"), None);
        assert_eq!(display_path("https://example.test/tmp"), None);
        assert_eq!(display_path("/home/ana"), None);
        assert_eq!(display_path(""), None);
        assert_eq!(display_path("file:"), None);
        assert_eq!(display_path("file:/tmp"), None);
        assert_eq!(display_path("file://"), None);
        assert_eq!(display_path("file://relative"), None);
    }

    #[test]
    fn drops_query_fragment_and_control_characters() {
        assert_eq!(
            display_path("file:///tmp/x?host=remote#top"),
            Some("/tmp/x".into())
        );
        assert_eq!(
            display_path("file:///tmp/a\u{7}b\u{1b}[31m"),
            Some("/tmp/ab[31m".into())
        );
        // A newline cannot be smuggled into a tooltip or a label.
        assert_eq!(
            display_path("file:///tmp/x\nhttps://evil.test"),
            Some("/tmp/xhttps://evil.test".into())
        );
    }

    #[test]
    fn replaces_invalid_utf8_instead_of_dropping_the_path() {
        let path = display_path("file:///tmp/%FF").unwrap();
        assert!(path.starts_with("/tmp/"));
        assert!(path.contains('\u{fffd}'));
    }

    #[test]
    fn keeps_the_tail_of_an_absurdly_long_path() {
        let long = format!("file:///{}{}", "../".repeat(MAX_PATH_CHARS), "end");
        let path = display_path(&long).unwrap();
        assert_eq!(path.chars().count(), MAX_PATH_CHARS);
        assert!(path.starts_with('…'));
        assert!(path.ends_with("end"));
    }
}
