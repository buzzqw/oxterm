//! Headless core of Oxterm.
//!
//! The binary uses these modules for its command line fast paths, and the
//! `oxterm-docgen` tool uses them to generate documentation. Nothing here may
//! depend on the GUI toolkit; see `src/headless/mod.rs`.

pub mod headless;
