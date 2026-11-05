# Oxterm prebuilt Linux binaries

This archive contains the Oxterm release binaries and the documentation needed
to run them. You do not need Rust, Cargo or the development packages: only the
GTK3 and VTE 2.91 runtime libraries must be installed.

## Contents

- `oxterm-<version>-linux-x86_64` or `oxterm-latest-linux-x86_64`:
  the terminal emulator (the GUI binary).
- `oxterm-broker-*-linux-x86_64`: the low-memory PTY broker. Keep it next to
  the GUI binary.
- `oxterm-askpass-*-linux-x86_64`: the helper used for stored passwords. Keep
  it next to the GUI binary.
- `manual.md`: the complete user manual.
- `THEME-LICENSES.md`: the licences of the bundled themes.
- `SHA256SUMS`: checksums for the files in this archive.
- `LICENSE`: the project licence (EUPL 1.2).

The three executables must stay in the same directory: Oxterm looks for the
broker and the askpass helper next to its own binary. Without them it still
works, but it falls back to `oxterm --broker` and asks for passwords in the tab.

## Requirements

- Linux x86_64.
- GTK3 and VTE 2.91 runtime libraries, VTE 0.70 or newer.
- `openssh` for remote sessions (optional).
- `xdg-utils` for opening notes and links with the desktop applications
  (optional).

Install the runtime libraries with your distribution:

```bash
# Debian / Ubuntu
sudo apt install libgtk-3-0 libvte-2.91-0

# Fedora
sudo dnf install gtk3 vte291

# Arch Linux
sudo pacman -S gtk3 vte3
```

## Run it

Extract the archive, make the binaries executable and start the GUI binary:

```bash
tar -xzf oxterm-*.tar.gz
chmod +x oxterm-*-linux-x86_64
./oxterm-latest-linux-x86_64        # rolling latest
./oxterm-<version>-linux-x86_64     # versioned release, e.g. oxterm-1.1.27-linux-x86_64
```

`oxterm --help` lists the command line options. The manual explains the
interface, the `Connections` menu, remote sessions, configuration and
troubleshooting.

## Install it

Put all three binaries in a directory in `PATH`, for example:

```bash
mkdir -p ~/.local/bin
install -m 0755 oxterm-*-linux-x86_64 ~/.local/bin/
```

Then rename them to the names other tools expect:

```bash
cd ~/.local/bin
mv oxterm-latest-linux-x86_64 oxterm
mv oxterm-broker-latest-linux-x86_64 oxterm-broker
mv oxterm-askpass-latest-linux-x86_64 oxterm-askpass
```

Make sure `~/.local/bin` is in your `PATH`, then run `oxterm`. The versioned
archive uses the same names with `-<version>` before `-linux-x86_64`.

Alternatively, install the files in `/usr/local/bin`:

```bash
sudo install -m 0755 oxterm-latest-linux-x86_64 /usr/local/bin/oxterm
sudo install -m 0755 oxterm-broker-latest-linux-x86_64 /usr/local/bin/oxterm-broker
sudo install -m 0755 oxterm-askpass-latest-linux-x86_64 /usr/local/bin/oxterm-askpass
```

## Verify the download

From the directory where you extracted the archive:

```bash
sha256sum -c SHA256SUMS
```

The `SHA256SUMS` published next to the archive on the releases page also
contains the checksum of the `.tar.gz` file itself.

## Documentation and licence

Read `manual.md` for the user manual. The project licence is in `LICENSE`
(European Union Public Licence 1.2); the bundled themes are covered by
`THEME-LICENSES.md`.

- Project and issues: <https://github.com/buzzqw/oxterm>
- Releases: <https://github.com/buzzqw/oxterm/releases>
- Manual: <https://github.com/buzzqw/oxterm/blob/master/manual.md>
