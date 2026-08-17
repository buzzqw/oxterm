# Project guidelines

## Git

- Commit and push **always as user `buzzqw`** (GitHub owner of `origin`,
  https://github.com/buzzqw/terust.git). This is the user to use for every
  git operation in this project.
- Do **not** add Junie (or any other) co-author trailer to commits.

## Build

- After every compilation, always copy the definitive release binary to
  `/home/azanzani/terust/terust-linux-x86-64` (see `build.sh` and
  `.githooks/post-commit`).
