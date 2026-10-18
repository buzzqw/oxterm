# AUR packaging

Files in this directory are the source of truth for the
[AUR](https://aur.archlinux.org) package `oxterm`.

- `PKGBUILD` — builds Oxterm from the upstream Git tag `v$pkgver`.
- `.SRCINFO` — generated metadata consumed by the AUR web interface and helpers.

## Installing from the AUR

Once the package is published:

```sh
yay -S oxterm        # or: paru -S oxterm
```

## Why `options=('!lto')`

Cargo already builds the release profile with fat LTO (`Cargo.toml`). Letting
makepkg add `-flto` to `CFLAGS`/`LDFLAGS` as well makes the bundled `sqlite3`
static archive fail to link (`undefined symbol: sqlite3_*`) on toolchains that
combine GCC LTO with `lld`. The package therefore opts out of makepkg's LTO and
leaves the link step to Cargo.

## Releasing a new version

1. Bump `pkgver` in `PKGBUILD` to the upstream tag (without the leading `v`)
   and reset `pkgrel=1` when the version changes.
2. Regenerate the metadata:

   ```sh
   cd packaging/aur
   makepkg --printsrcinfo > .SRCINFO
   ```

3. Build the package locally to make sure the tag is fetchable and the sources
   compile:

   ```sh
   makepkg -f --cleanbuild
   ./oxterm --version
   ```

4. Push the two files to the AUR repository (requires an AUR account with an
   SSH key configured at <https://aur.archlinux.org/account>):

   ```sh
   git clone ssh://aur@aur.archlinux.org/oxterm.git /tmp/aur-oxterm
   cp PKGBUILD .SRCINFO /tmp/aur-oxterm/
   cd /tmp/aur-oxterm
   git add PKGBUILD .SRCINFO
   git commit -m "upgpkg: oxterm 1.1.11-1"
   git push
   ```

The CI workflow `.github/workflows/aur.yml` validates on every change that
`.SRCINFO` is up to date, that `pkgver` matches `Cargo.toml`, and that the
package builds on Arch Linux.
