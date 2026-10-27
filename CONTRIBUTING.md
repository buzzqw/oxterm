# Contributing

Before a commit that changes Rust source or build configuration, the project
must pass a release build. The repository hook `.githooks/pre-commit` runs
`cargo build --release --locked` automatically and rejects the commit if the
build fails.

Run `./setup.sh` once after cloning to configure Git to use the repository
hooks. The `post-commit` hook then updates the release executable in
`oxterm-linux-x86-64`, and `post-merge`/`post-rewrite` rebuild after a `git
pull` (merge or rebase) and refresh an existing per-user installation.

Screenshots used by the README live in `docs/screenshots/`. After a change that
affects the UI, refresh them with `scripts/capture-screenshots.sh` (it needs
Xvnc, xdotool, ImageMagick and xfwm4) and commit the updated PNGs.

## Source layout

`src/headless/` is the display-free core: command line parsing, control-command
parsing, the settings schema and validation, JSON framing, diagnostics and the
pure VI copy-mode helpers. It must not import `gtk`, `gdk`, `glib` or VTE;
`tests/headless.rs` enforces the boundary. New pure logic belongs there so it can
be tested without X11, and `cargo test --lib` stays fast.

The GUI lives in the remaining modules and in `src/main.rs`. The binary
re-exports the core as `crate::headless` for those modules.

## Generated documentation

`--help`, the OPTIONS section of `packaging/oxterm.1`, the bash/fish/zsh
completions and `docs/settings.md` are generated from `src/headless/spec.rs`.
After changing a command line option, a setting or its description, run:

```bash
cargo run --bin oxterm-docgen
```

and commit the result. CI runs `cargo run --bin oxterm-docgen -- --check` and
fails when a generated file is stale.
