//! Hover decoders for numbers that are tedious to interpret by hand.
//!
//! The terminal only shows a tooltip when the token is unambiguous enough:
//! a 10/13-digit Unix epoch in a plausible range, or a known HTTP/errno code
//! on a line that actually talks about errors/status. This keeps ordinary
//! numbers in logs from spawning tooltips everywhere.

/// Decode a 10-digit seconds or 13-digit milliseconds timestamp.
///
/// The accepted range is 2000-01-01 through 2100-01-01, which rejects IDs and
/// random counters while still covering realistic log data.
pub fn epoch_tooltip(token: &str) -> Option<String> {
    if !token.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (seconds, milliseconds) = match token.len() {
        10 => (token.parse::<i64>().ok()?, false),
        13 => (token.parse::<i64>().ok()? / 1000, true),
        _ => return None,
    };
    if !(946_684_800..=4_102_444_800).contains(&seconds) {
        return None;
    }
    let time = seconds as libc::time_t;
    let mut local = unsafe { std::mem::zeroed::<libc::tm>() };
    if unsafe { libc::localtime_r(&time, &mut local) }.is_null() {
        return None;
    }
    let mut buffer = [0 as libc::c_char; 64];
    let format = b"%Y-%m-%d %H:%M:%S %Z\0";
    let length = unsafe {
        libc::strftime(
            buffer.as_mut_ptr(),
            buffer.len(),
            format.as_ptr() as *const _,
            &local,
        )
    };
    if length == 0 {
        return None;
    }
    let formatted = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy();
    Some(if milliseconds {
        format!("Epoch {} ms → {}", token, formatted)
    } else {
        format!("Epoch {} → {}", token, formatted)
    })
}

fn http_status(code: u16) -> Option<&'static str> {
    Some(match code {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        418 => "I'm a teapot",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        _ => return None,
    })
}

fn errno(code: u16) -> Option<(&'static str, &'static str)> {
    Some(match code {
        1 => ("EPERM", "Operation not permitted"),
        2 => ("ENOENT", "No such file or directory"),
        5 => ("EIO", "Input/output error"),
        11 => ("EAGAIN", "Resource temporarily unavailable"),
        13 => ("EACCES", "Permission denied"),
        17 => ("EEXIST", "File exists"),
        28 => ("ENOSPC", "No space left on device"),
        32 => ("EPIPE", "Broken pipe"),
        98 => ("EADDRINUSE", "Address already in use"),
        99 => ("EADDRNOTAVAIL", "Cannot assign requested address"),
        104 => ("ECONNRESET", "Connection reset by peer"),
        110 => ("ETIMEDOUT", "Connection timed out"),
        111 => ("ECONNREFUSED", "Connection refused"),
        113 => ("EHOSTUNREACH", "No route to host"),
        _ => return None,
    })
}

fn error_context(line: &str) -> bool {
    let line = line.to_lowercase();
    [
        "http", "status", "errno", "error", "failed", "failure", "refused", "timeout",
    ]
    .iter()
    .any(|needle| line.contains(needle))
}

/// Build the hover text for `token` on `line`, if it is worth decoding.
pub fn hover_tooltip(token: &str, line: &str) -> Option<String> {
    if let Some(epoch) = epoch_tooltip(token) {
        return Some(epoch);
    }
    if token.len() != 3 || !token.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let code = token.parse::<u16>().ok()?;
    if !error_context(line) {
        return None;
    }
    if let Some(reason) = http_status(code) {
        return Some(format!("HTTP {} {}", code, reason));
    }
    errno(code).map(|(name, description)| format!("{} (errno {}): {}", name, code, description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_plausible_epochs_only() {
        let text = epoch_tooltip("1730458800").unwrap();
        assert!(text.contains("2024-11"), "{text}");
        let text = epoch_tooltip("1730458800000").unwrap();
        assert!(text.contains("2024-11"), "{text}");
        assert!(epoch_tooltip("12345678").is_none());
        assert!(epoch_tooltip("9999999999").is_none());
        assert!(epoch_tooltip("17304588001").is_none());
        assert!(epoch_tooltip("not-a-number").is_none());
    }

    #[test]
    fn decodes_http_and_errno_in_context() {
        assert_eq!(
            hover_tooltip("504", "HTTP/1.1 504 from upstream").as_deref(),
            Some("HTTP 504 Gateway Timeout")
        );
        assert_eq!(
            hover_tooltip("111", "connect: errno=111").as_deref(),
            Some("ECONNREFUSED (errno 111): Connection refused")
        );
        assert!(hover_tooltip("504", "the answer is 504").is_none());
        assert!(hover_tooltip("999", "error 999").is_none());
    }
}
