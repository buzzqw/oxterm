# Contributing

Before a commit that changes Rust source or build configuration, the project
must pass a release build. The repository hook `.githooks/pre-commit` runs
`cargo build --release` automatically and rejects the commit if the build
fails.

Run `./setup.sh` once after cloning to configure Git to use the repository
hooks. The existing `post-commit` hook then updates the release executable in
`TRust-linux-x86-64`.
