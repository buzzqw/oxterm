# Contributing

Before a commit that changes Rust source or build configuration, the project
must pass a release build. The repository hook `.githooks/pre-commit` runs
`cargo build --release --locked` automatically and rejects the commit if the
build fails.

Run `./setup.sh` once after cloning to configure Git to use the repository
hooks. The `post-commit` hook then updates the release executable in
`oxterm-linux-x86-64`, and `post-merge`/`post-rewrite` rebuild after a `git
pull` (merge or rebase) and refresh an existing per-user installation.
