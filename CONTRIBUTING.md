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

## Terminal behaviour

`docs/compatibility.md` records what Oxterm promises about terminal behaviour,
what it delegates to VTE, and what is out of scope. A change that alters how a
sequence or the OSC 133 protocol behaves needs an entry there and a test: unit
tests live next to the code in `src/headless/`, and recorded sessions are
replayed by `tests/osc133_conformance.rs` from `tests/fixtures/osc133/`.

## Themes

`assets/themes.json` is generated from the MIT-licensed
[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)
collection by `scripts/update-themes.py`, which bundles only themes whose
licence permits redistribution and refreshes `themes/licenses.md`. Oxterm's own
palettes stay in `src/headless/themes.rs` and win over a bundled theme with the
same name. To update the pack:

```bash
python3 scripts/update-themes.py /path/to/iTerm2-Color-Schemes
```
