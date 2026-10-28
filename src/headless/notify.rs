//! Policy for "command finished" desktop notifications.
//!
//! Notifications should be useful, not noisy: a command that finishes while you
//! are watching it does not need a popup, and neither does `ls`. Failures are
//! the exception, because they are worth interrupting for.

/// Everything needed to decide whether a completion deserves a notification.
pub struct Notification<'a> {
    /// Master switch (`bell_notification`).
    pub enabled: bool,
    /// Whether the terminal currently has keyboard focus.
    pub focused: bool,
    /// Skip notifications while focused (`notify_only_unfocused`).
    pub only_unfocused: bool,
    /// `None` when the shell closed the command without reporting a status.
    pub exit_code: Option<i64>,
    /// How long the command ran, when the shell integration reported it.
    pub duration_ms: Option<i64>,
    /// Minimum runtime for successful commands (`notify_min_seconds`).
    pub min_seconds: i64,
    /// Always notify on failure, even for fast commands (`notify_on_failure`).
    pub notify_on_failure: bool,
    pub command: &'a str,
    pub cwd: &'a str,
}

pub fn should_notify(notification: &Notification<'_>) -> bool {
    if !notification.enabled {
        return false;
    }
    if notification.only_unfocused && notification.focused {
        return false;
    }
    let failed = matches!(notification.exit_code, Some(code) if code != 0);
    if failed && notification.notify_on_failure {
        return true;
    }
    let minimum = notification.min_seconds.max(0) * 1000;
    notification.duration_ms.unwrap_or(0) >= minimum
}

/// One-line title: failures are called out explicitly.
pub fn title(notification: &Notification<'_>) -> &'static str {
    if matches!(notification.exit_code, Some(code) if code != 0) {
        "Command failed"
    } else {
        "Command finished"
    }
}

/// Notification body: what ran, how it ended, where, and how long it took.
pub fn body(notification: &Notification<'_>) -> String {
    let command = notification.command.trim();
    let command = if command.is_empty() {
        "Command".to_string()
    } else {
        format!("\"{}\"", shorten(command, 90))
    };
    let status = match notification.exit_code {
        Some(0) => "finished".to_string(),
        Some(code) => format!("failed with exit {code}"),
        // Never report an unknown status as a success.
        None => "finished without reporting a status".to_string(),
    };
    let mut text = format!("{command} {status}");
    if let Some(duration) = notification.duration_ms {
        text.push_str(&format!(" after {}", format_duration(duration)));
    }
    let cwd = notification.cwd.trim();
    if !cwd.is_empty() {
        text.push('\n');
        text.push_str(&shorten(cwd, 120));
    }
    text
}

pub fn format_duration(milliseconds: i64) -> String {
    let seconds = milliseconds.max(0) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

fn shorten(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification<'a>() -> Notification<'a> {
        Notification {
            enabled: true,
            focused: false,
            only_unfocused: true,
            exit_code: Some(0),
            duration_ms: Some(30_000),
            min_seconds: 10,
            notify_on_failure: true,
            command: "make test",
            cwd: "/src/project",
        }
    }

    #[test]
    fn hides_when_disabled_or_focused() {
        let mut n = notification();
        n.enabled = false;
        assert!(!should_notify(&n));
        let mut n = notification();
        n.focused = true;
        assert!(!should_notify(&n));
        let mut n = notification();
        n.focused = true;
        n.command = "ignored";
        n.duration_ms = Some(1_000_000);
        assert!(!should_notify(&n), "focused terminal stays quiet");
    }

    #[test]
    fn skips_quick_successes_but_never_quiet_failures() {
        let mut n = notification();
        n.duration_ms = Some(3_000);
        assert!(!should_notify(&n), "3s is below the 10s threshold");
        n.exit_code = Some(1);
        assert!(should_notify(&n), "failures bypass the duration threshold");
        n.notify_on_failure = false;
        assert!(!should_notify(&n));
        n.duration_ms = Some(10_000);
        assert!(should_notify(&n));
        n.min_seconds = 0;
        n.duration_ms = Some(0);
        assert!(should_notify(&n));
    }

    #[test]
    fn body_names_the_command_status_duration_and_directory() {
        let n = notification();
        assert_eq!(title(&n), "Command finished");
        assert_eq!(body(&n), "\"make test\" finished after 30s\n/src/project");
        let mut failed = notification();
        failed.exit_code = Some(2);
        failed.duration_ms = Some(72_000);
        assert_eq!(title(&failed), "Command failed");
        assert_eq!(
            body(&failed),
            "\"make test\" failed with exit 2 after 1m 12s\n/src/project"
        );
    }

    #[test]
    fn an_unknown_status_is_neither_success_nor_failure() {
        let mut n = notification();
        n.exit_code = None;
        n.duration_ms = Some(3_000);
        assert!(
            !should_notify(&n),
            "an unknown status must not bypass the duration rule"
        );
        n.duration_ms = Some(30_000);
        assert!(should_notify(&n), "long commands still notify");
        assert_eq!(title(&n), "Command finished");
        assert!(
            body(&n).contains("finished without reporting a status"),
            "body: {}",
            body(&n)
        );
    }

    #[test]
    fn formats_durations_for_humans() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(999), "0s");
        assert_eq!(format_duration(1_500), "1s");
        assert_eq!(format_duration(59_000), "59s");
        assert_eq!(format_duration(60_000), "1m 0s");
        assert_eq!(format_duration(3_723_000), "1h 2m");
    }

    #[test]
    fn truncates_long_commands_on_character_boundaries() {
        let long = "à".repeat(120);
        let mut n = notification();
        n.command = &long;
        let text = body(&n);
        assert!(text.starts_with('"'));
        assert!(text.contains('…'));
    }
}
