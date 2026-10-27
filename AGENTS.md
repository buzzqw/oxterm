# Project instructions

## Git workflow

- Always commit and push the changes when a task is done. Do not leave work
  uncommitted or unpushed; do not wait to be asked.
- Push to `origin` on the current branch (`master`) after committing.
- Use concise conventional-style commit messages (for example
  `fix: ...`, `chore: ...`, `feat: ...`), matching the existing history.

## Architecture and generated files

- Put display-free logic in `src/headless/` and keep it free of GTK/VTE; the
  `tests/headless.rs` boundary test enforces this.
- Command line options, settings, help text, man page, completions and
  `docs/settings.md` come from `src/headless/spec.rs`. After changing them run
  `cargo run --bin oxterm-docgen` and commit the regenerated files.
