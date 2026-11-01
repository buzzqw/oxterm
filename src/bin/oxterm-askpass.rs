//! `SSH_ASKPASS` helper for connections whose password lives in the keyring.
//!
//! `ssh` runs this with the prompt as its argument and reads the answer from
//! stdout. It answers on its own only for a password or one-time code prompt that
//! names the connection's target, once per `ssh` process (see
//! `headless::askpass`). Every
//! other prompt is shown on the controlling terminal, which `ssh` shares with
//! this process, so host-key checks and passphrases stay with the person. With
//! no terminal the helper fails, which `ssh` treats as a refusal.
//!
//! It links neither GTK nor VTE: it starts for a single prompt and must work
//! wherever `ssh` does.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::AsRawFd;
use std::process::ExitCode;

use oxterm::headless::askpass::{self, Decision};

fn main() -> ExitCode {
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let target = std::env::var(askpass::ENV_TARGET).unwrap_or_default();
    let id = std::env::var(askpass::ENV_ID).unwrap_or_default();
    let totp = std::env::var_os(askpass::ENV_TOTP).is_some();
    let answer = match askpass::decide(&prompt, &target, totp) {
        Decision::UseStoredPassword => stored_password(&id).or_else(|| ask(&prompt, false)),
        Decision::UseTotpCode => stored_code(&id).or_else(|| ask(&prompt, false)),
        Decision::AskVisible => ask(&prompt, true),
        Decision::AskHidden => ask(&prompt, false),
    };
    match answer {
        Some(answer) => {
            let mut stdout = std::io::stdout().lock();
            if writeln!(stdout, "{}", answer).is_err() {
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

fn stored_password(id: &str) -> Option<String> {
    if id.is_empty() || !first_attempt("password") {
        return None;
    }
    askpass::lookup_secret(id)
}

fn stored_code(id: &str) -> Option<String> {
    if id.is_empty() || !first_attempt("totp") {
        return None;
    }
    askpass::lookup_totp_code(id)
}

/// The parent is the `ssh` process that asked.
fn first_attempt(kind: &str) -> bool {
    askpass::first_attempt(std::os::unix::process::parent_id(), kind)
}

/// Ask on `/dev/tty`; `None` when there is no terminal or it is closed.
fn ask(prompt: &str, echo: bool) -> Option<String> {
    let mut tty: File = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let fd = tty.as_raw_fd();
    let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: `fd` is open for the whole function and `saved` is only read after
    // `tcgetattr` initialized it.
    let saved = unsafe {
        if libc::tcgetattr(fd, saved.as_mut_ptr()) != 0 {
            return None;
        }
        saved.assume_init()
    };
    if !echo {
        let mut hidden = saved;
        hidden.c_lflag &= !libc::ECHO;
        hidden.c_lflag |= libc::ECHONL;
        // SAFETY: as above.
        unsafe {
            libc::tcsetattr(fd, libc::TCSAFLUSH, &hidden);
        }
    }
    let _ = tty.write_all(prompt.as_bytes());
    let _ = tty.flush();
    let mut line = String::new();
    let read = BufReader::new(&tty).read_line(&mut line);
    if !echo {
        // SAFETY: as above.
        unsafe {
            libc::tcsetattr(fd, libc::TCSAFLUSH, &saved);
        }
    }
    match read {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
    }
}
