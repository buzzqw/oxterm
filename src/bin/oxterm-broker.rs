//! Display-free PTY broker.
//!
//! `oxterm` spawns this executable for every session when it is installed next
//! to the GUI binary. It shares `remote.rs` with the GUI but links neither GTK
//! nor VTE, so an idle broker maps a couple of megabytes instead of the ~20 MB
//! of GUI libraries. Distributions and the release tarball install it
//! alongside `oxterm`; a lone `oxterm` binary falls back to `oxterm --broker`.

// Most of `remote.rs` is the client side (`--list`, attach, relay), which this
// binary deliberately never runs but must compile because the broker and the
// client share frame definitions and socket helpers.
#![allow(dead_code)]

pub use oxterm::headless;

#[path = "../remote.rs"]
mod remote;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match remote::parse_cli_mode(&args) {
        Some(Ok(remote::CliMode::Broker(path, id))) => {
            ExitCode::from(remote::run_cli(remote::CliMode::Broker(path, id)) as u8)
        }
        Some(Ok(_)) => {
            eprintln!("oxterm-broker: only the internal --broker mode is supported");
            ExitCode::FAILURE
        }
        Some(Err(error)) => {
            eprintln!("oxterm-broker: {error}");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("usage: oxterm-broker --broker SOCKET SESSION_ID");
            ExitCode::FAILURE
        }
    }
}
