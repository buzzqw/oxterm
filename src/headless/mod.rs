//! Oxterm's headless core.
//!
//! Everything in this module is independent of GTK, VTE and the display: it can
//! be exercised by unit tests, by the `oxterm-docgen` binary and by the command
//! line fast paths in `main` without initialising a GUI. Keep it that way: no
//! `gtk`, `gdk`, `glib` or `zoha_vte` imports are allowed under `headless/`.
//! `tests/headless.rs` enforces the boundary.

pub mod cli;
pub mod control_cli;
pub mod diagnostics;
pub mod framing;
pub mod settings;
pub mod spec;
pub mod text;
pub mod vi;
