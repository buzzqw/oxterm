//! Conformance tests for the OSC 133 FIFO protocol.
//!
//! The ghostty project audits terminal behaviour by comparing every sequence
//! against xterm and keeping the evidence — unit tests, a replay script and
//! prose — in one place. Oxterm does not parse escape sequences itself (VTE
//! does), but it does own the protocol its shell integration writes to a
//! private FIFO. This file is the equivalent artifact for that protocol: a
//! recorded session in `tests/fixtures/osc133/`, replayed at several read
//! sizes, checked against the events the rest of the terminal expects.
//!
//! `docs/compatibility.md` is the prose half and documents the same rules.

use std::path::Path;

use oxterm::headless::osc133::{self, Event, LineBuffer};

/// The event stream the fixture must produce, in order.
#[derive(Debug, PartialEq)]
enum Seen {
    Prompt,
    Command(String),
    End(Option<i64>),
}

impl From<Event<'_>> for Seen {
    fn from(event: Event<'_>) -> Self {
        match event {
            Event::PromptStart => Seen::Prompt,
            Event::CommandStart(command) => Seen::Command(command.to_string()),
            Event::CommandEnd(status) => Seen::End(status),
        }
    }
}

fn expected() -> Vec<Seen> {
    use Seen::{Command, End, Prompt};
    vec![
        // The integration script reports A as soon as it loads.
        Prompt,
        // A command that prints nothing.
        Command("true".into()),
        End(Some(0)),
        Prompt,
        Command("git status".into()),
        End(Some(0)),
        Prompt,
        // Reversal and wide characters must survive the FIFO untouched.
        Command("grep -R \"città 🚀\" src".into()),
        End(Some(1)),
        Prompt,
        // A missing status is not success.
        Command("false".into()),
        End(None),
        Prompt,
        // Text that looks like protocol lines stays command text.
        Command("printf 'A\\nD1\\n'".into()),
        End(Some(0)),
        Prompt,
        // Padding around the command line is trimmed.
        Command("ls -l ~/oxterm".into()),
        End(Some(0)),
        Prompt,
    ]
}

fn fixture() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/osc133/session.fifo");
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// Feed the stream in reads of `chunk` bytes, as the GLib source would.
fn replay(stream: &[u8], chunk: usize) -> Vec<Seen> {
    let mut buffer = LineBuffer::new();
    let mut seen = Vec::new();
    for bytes in stream.chunks(chunk) {
        for line in buffer.push(bytes) {
            if let Some(event) = Event::parse(&line) {
                seen.push(event.into());
            }
        }
    }
    assert!(
        !buffer.take_overflow(),
        "the fixture must fit in the {} KiB line buffer",
        osc133::MAX_BUFFER / 1024
    );
    seen
}

#[test]
fn replays_the_recorded_session_at_every_read_size() {
    let stream = fixture();
    let expected = expected();
    // One byte at a time is the worst case for the line buffer: every read
    // splits a line. The whole stream is the opposite. The sizes in between
    // cover reads that end exactly on a newline and just after one.
    for chunk in [1, 2, 3, 7, 64, 4096, stream.len()] {
        assert_eq!(replay(&stream, chunk), expected, "read size {chunk}");
    }
}

#[test]
fn the_fixture_is_pure_protocol_plus_comments() {
    let stream = fixture();
    let text = String::from_utf8(stream).expect("the fixture is UTF-8 text");
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        assert!(
            Event::parse(line).is_some(),
            "fixture line {line:?} is neither a comment nor an event"
        );
    }
}

#[test]
fn a_full_buffer_of_junk_does_not_lose_the_next_event() {
    // A program that opens the FIFO and never sends a newline must not wedge
    // the reader, and the shell must keep working once it sends whole lines.
    let mut buffer = LineBuffer::new();
    let junk = vec![b'x'; osc133::MAX_BUFFER + 1];
    assert!(buffer.push(&junk).is_empty());
    assert!(buffer.take_overflow());
    assert_eq!(buffer.push(b"A\nCtrue\nD0\n"), vec!["A", "Ctrue", "D0"]);
}

/// The prose in `docs/compatibility.md` states the limits; this keeps the
/// numbers from drifting away from the constants.
#[test]
fn documented_limits_match_the_constants() {
    assert_eq!(osc133::MAX_BUFFER % 1024, 0);
    assert_eq!(osc133::MAX_COMMAND % 1024, 0);

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/compatibility.md");
    let doc = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    for (name, bytes) in [
        ("OSC 133 FIFO line buffer", osc133::MAX_BUFFER),
        ("OSC 133 command text", osc133::MAX_COMMAND),
    ] {
        let expected = format!("{name} | {} KiB", bytes / 1024);
        assert!(
            doc.contains(&expected),
            "{} must document {expected:?}",
            path.display()
        );
    }
}
