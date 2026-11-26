//! Oxterm's headless core.
//!
//! Everything in this module is independent of GTK, VTE and the display: it can
//! be exercised by unit tests, by the `oxterm-docgen` binary and by the command
//! line fast paths in `main` without initialising a GUI. Keep it that way: no
//! `gtk`, `gdk`, `glib` or `zoha_vte` imports are allowed under `headless/`.
//! `tests/headless.rs` enforces the boundary.

pub mod acp;
pub mod agent;
pub mod askpass;
pub mod bookmarks;
pub mod cli;
pub mod connection_import;
pub mod connection_uri;
pub mod connections;
pub mod control_cli;
pub mod decode;
pub mod desktop;
pub mod diagnostics;
pub mod events;
pub mod expect;
pub mod extensions;
pub mod file_uri;
pub mod framing;
pub mod mcp;
pub mod notes_prompt;
pub mod notify;
pub mod osc133;
pub mod password;
pub mod paths;
pub mod persistence;
pub mod preconnect;
pub mod reachability;
pub mod remote_files;
pub mod settings;
pub mod spec;
pub mod ssh_config;
pub mod ssh_sync;
pub mod sync;
pub mod text;
pub mod themes;
pub mod totp;
pub mod transfer_queue;
pub mod triggers;
pub mod tunnels;
pub mod vi;
pub mod workspace;
pub mod workspaces;
