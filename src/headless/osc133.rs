//! The OSC 133 event protocol Oxterm's shell integration writes to its FIFO.
//!
//! Oxterm does not read prompt information out of VTE: the start-up scripts
//! installed for Bash, Zsh and Fish write one event per line to a private FIFO.
//! That keeps the two things VTE does not expose — the command line itself and
//! the exit status of a command that has already printed its next prompt — and
//! lets anything able to open the FIFO feed the same events.
//!
//! The wire format is one line per event, `\n` terminated:
//!
//! | Line | Meaning |
//! | --- | --- |
//! | `A` | prompt start; payload after the `A` is ignored |
//! | `C<text>` | command start, `<text>` is the command line |
//! | `D<n>` | command end, `<n>` is the exit status |
//!
//! Lines are trimmed before parsing, so `A`, ` A ` and `A\r` are the same
//! event, blank lines and unknown letters are dropped, and a `D` that carries
//! no number reports "no status" rather than success. Leading whitespace is
//! also removed from the command text, which is capped at [`MAX_COMMAND`]
//! characters.
//!
//! Parsing and buffering live here, not in the terminal widget, so the whole
//! protocol can be exercised without a display:
//! `tests/osc133_conformance.rs` replays a recorded session from
//! `tests/fixtures/osc133/`.

use super::text;

/// Largest unterminated line the FIFO buffer will hold. A shell writing more
/// than this without a newline is broken or hostile; the line is dropped.
pub const MAX_BUFFER: usize = 64 * 1024;

/// A command line is stored in the history database and shown in the UI, so a
/// hostile or broken integration cannot insert megabytes.
pub const MAX_COMMAND: usize = 8 * 1024;

/// One event as reported by the shell integration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event<'a> {
    PromptStart,
    CommandStart(&'a str),
    /// `None` when the shell reported no status, for example after a command
    /// that was cancelled before `$?` was captured.
    CommandEnd(Option<i64>),
}

impl<'a> Event<'a> {
    /// Parse one trimmed FIFO line. `None` means the line is not an event.
    pub fn parse(line: &'a str) -> Option<Self> {
        let mut chars = line.chars();
        let event = match chars.next()? {
            'A' => Event::PromptStart,
            'C' => Event::CommandStart(text::truncate_to_chars(chars.as_str().trim(), MAX_COMMAND)),
            'D' => Event::CommandEnd(parse_exit_status(chars.as_str())),
            _ => return None,
        };
        Some(event)
    }

    /// Prompt ends and command ends are only meaningful once VTE has consumed
    /// the output that precedes them, so the terminal widget handles them after
    /// a short delay. Command starts must be handled immediately: their row
    /// comes from the pending keystroke, not from the output.
    pub fn is_deferred(&self) -> bool {
        matches!(self, Event::PromptStart | Event::CommandEnd(_))
    }
}

/// Accept both the private `D<n>` spelling and the standard `D;<n>` one, so a
/// hand-written integration cannot silently turn a failure into a success.
fn parse_exit_status(status: &str) -> Option<i64> {
    let status = status.trim();
    let status = status.strip_prefix(';').unwrap_or(status).trim();
    if status.is_empty() {
        None
    } else {
        status.parse().ok()
    }
}

/// Reassembles FIFO reads into whole lines.
///
/// A single `read(2)` can return half a line or several lines at once, and no
/// single line may grow past [`MAX_BUFFER`]: when that happens the line is
/// dropped and [`take_overflow`](Self::take_overflow) reports it so the caller
/// can log it exactly once.
#[derive(Debug, Default)]
pub struct LineBuffer {
    pending: Vec<u8>,
    overflowed: bool,
}

impl LineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append bytes read from the FIFO and return the events they completed,
    /// already trimmed, with blank lines dropped.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(offset) = self.pending[start..].iter().position(|&b| b == b'\n') {
            let end = start + offset;
            let line = String::from_utf8_lossy(&self.pending[start..end]);
            let line = line.trim();
            if !line.is_empty() {
                lines.push(line.to_string());
            }
            start = end + 1;
        }
        self.pending.drain(..start);
        if self.pending.len() > MAX_BUFFER {
            self.pending.clear();
            self.overflowed = true;
        }
        lines
    }

    /// True once a line was dropped for exceeding [`MAX_BUFFER`]. Clears the
    /// flag, so each overflow is logged once.
    pub fn take_overflow(&mut self) -> bool {
        std::mem::take(&mut self.overflowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events<'a>(lines: &'a [&'a str]) -> Vec<Event<'a>> {
        lines.iter().filter_map(|line| Event::parse(line)).collect()
    }

    #[test]
    fn parses_the_three_events() {
        assert_eq!(
            events(&["A", "Cecho hello", "D0"]),
            vec![
                Event::PromptStart,
                Event::CommandStart("echo hello"),
                Event::CommandEnd(Some(0)),
            ]
        );
    }

    #[test]
    fn keeps_the_command_line_verbatim() {
        assert_eq!(
            Event::parse(r#"Cgit commit -m "città 🚀" #2"#),
            Some(Event::CommandStart(r#"git commit -m "città 🚀" #2"#))
        );
        // The command may contain the letters that introduce other events.
        assert_eq!(Event::parse("CA; D1"), Some(Event::CommandStart("A; D1")));
    }

    #[test]
    fn trims_whitespace_around_the_command() {
        assert_eq!(
            Event::parse("C   ls -l  "),
            Some(Event::CommandStart("ls -l"))
        );
        assert_eq!(Event::parse("C"), Some(Event::CommandStart("")));
        assert_eq!(Event::parse("C \t "), Some(Event::CommandStart("")));
    }

    #[test]
    fn caps_the_command_on_a_character_boundary() {
        let long = format!("C{}", "à".repeat(MAX_COMMAND + 10));
        let Some(Event::CommandStart(command)) = Event::parse(&long) else {
            panic!("expected a command start");
        };
        assert_eq!(command.chars().count(), MAX_COMMAND);
        assert!(command.ends_with('à'));
    }

    #[test]
    fn reads_the_exit_status_in_both_spellings() {
        assert_eq!(Event::parse("D0"), Some(Event::CommandEnd(Some(0))));
        assert_eq!(Event::parse("D130"), Some(Event::CommandEnd(Some(130))));
        assert_eq!(Event::parse("D;127"), Some(Event::CommandEnd(Some(127))));
        assert_eq!(Event::parse("D 42 "), Some(Event::CommandEnd(Some(42))));
    }

    #[test]
    fn a_missing_status_is_not_success() {
        assert_eq!(Event::parse("D"), Some(Event::CommandEnd(None)));
        assert_eq!(Event::parse("D;"), Some(Event::CommandEnd(None)));
        assert_eq!(Event::parse("Dnope"), Some(Event::CommandEnd(None)));
        assert_eq!(Event::parse("D1x"), Some(Event::CommandEnd(None)));
    }

    #[test]
    fn ignores_everything_that_is_not_an_event() {
        for line in ["", "B", "Z", "133;A", "a", "c", "d", "\0", "€"] {
            assert_eq!(Event::parse(line), None, "parsed {line:?}");
        }
    }

    #[test]
    fn prompt_and_command_end_are_deferred() {
        assert!(!Event::parse("Cmake").unwrap().is_deferred());
        assert!(Event::parse("A").unwrap().is_deferred());
        assert!(Event::parse("D1").unwrap().is_deferred());
    }

    #[test]
    fn reassembles_reads_split_anywhere() {
        let stream = b"A\nCecho hello\nD0\nA\n";
        for split in 1..stream.len() {
            let mut buffer = LineBuffer::new();
            let mut lines = buffer.push(&stream[..split]);
            lines.extend(buffer.push(&stream[split..]));
            assert_eq!(
                lines,
                vec!["A", "Cecho hello", "D0", "A"],
                "split at {split}"
            );
        }
    }

    #[test]
    fn one_read_can_carry_several_lines() {
        let mut buffer = LineBuffer::new();
        assert_eq!(buffer.push(b"A\nCtrue\nD0\n"), vec!["A", "Ctrue", "D0"]);
        assert!(buffer.push(b"").is_empty());
    }

    #[test]
    fn accepts_crlf_and_drops_blank_lines() {
        let mut buffer = LineBuffer::new();
        assert_eq!(
            buffer.push(b"A\r\n\r\n  \r\nC true\r\nD0\r\n"),
            vec!["A", "C true", "D0"]
        );
    }

    #[test]
    fn survives_invalid_utf8() {
        let mut buffer = LineBuffer::new();
        let lines = buffer.push(b"Cecho \xff\xfe caf\xc3\xa9\n");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("Cecho "));
        assert!(lines[0].ends_with(" café"));
        assert!(lines[0].contains('\u{fffd}'));
    }

    #[test]
    fn drops_an_unterminated_line_that_exceeds_the_limit() {
        let mut buffer = LineBuffer::new();
        assert_eq!(buffer.push(b"A\n"), vec!["A"]);
        let huge = vec![b'C'; MAX_BUFFER + 1];
        assert!(buffer.push(&huge).is_empty());
        assert!(buffer.take_overflow());
        assert!(!buffer.take_overflow(), "the flag is cleared by reading it");
        // The reader recovers on the next complete line.
        assert_eq!(buffer.push(b"D0\n"), vec!["D0"]);
    }

    #[test]
    fn keeps_the_lines_bound_as_a_single_packet_under_the_limit() {
        let mut buffer = LineBuffer::new();
        let mut packet = vec![b'C'; MAX_BUFFER - 1];
        packet.push(b'\n');
        let lines = buffer.push(&packet);
        assert_eq!(lines.len(), 1);
        assert!(!buffer.take_overflow());
    }
}
