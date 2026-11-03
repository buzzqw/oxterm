# Compatibility and terminal conformance

Oxterm is a GTK3 front end over **VTE** (`vte-2.91`, through `zoha-vte`). VTE
parses the byte stream and implements CSI, DEC and OSC sequences, screen modes,
mouse reporting, selection and the character encodings. Oxterm does not
re-implement the parser and does not patch VTE's behaviour.

This document records the behaviour Oxterm owns, the oldest VTE it supports, and
what it deliberately does not implement. A claim belongs here when a test or a
reproducible check backs it up.

## What Oxterm owns and what it delegates

| Layer | Owner | Evidence |
| --- | --- | --- |
| OSC 133 events over the private FIFO (prompt, command, exit status) | Oxterm | `src/headless/osc133.rs`, `tests/osc133_conformance.rs` |
| Shell integration scripts and the `ssh`/`autossh` wrappers | Oxterm | `src/terminal/shell_scripts.rs`, [manual](../manual.md#shell-integration) |
| Window and tab titles, and their precedence | Oxterm | `forced_title` and `tab_custom_titles` in `src/window.rs` |
| OSC 8 hyperlinks: only `http`/`https` are opened | Oxterm | `security_tests` in `src/terminal.rs` |
| OSC 7 (shell-reported directory), display only | Oxterm | `src/headless/file_uri.rs` |
| URL detection and hint mode | Oxterm | `src/terminal.rs`, `src/terminal/hints.rs` |
| Paste safety: multi-line and control-heavy pastes ask first | Oxterm | `paste_checked_text` in `src/terminal.rs` |
| Control socket (`oxterm send`, `capture`, …) | Oxterm | `tests/control_api.rs` |
| Everything else in the byte stream | VTE | CI builds and smoke-tests against VTE 0.70 and the current Ubuntu release |

A sequence not mentioned here is neither promised nor forbidden: it follows the
VTE version the system provides. To diagnose a sequence problem, first reproduce
it in GNOME Terminal on the same machine. If it reproduces there, the cause is
most likely VTE's rather than Oxterm's, and the fix is a VTE upgrade or an
upstream report.

## The oldest supported VTE is 0.70

Oxterm builds and runs against VTE 0.70, the version in Debian 12 (bookworm). The
`vte-0-70` CI job builds on Debian 12 and runs `scripts/smoke-gui.sh` under Xvfb,
so the promise covers the run time and not just the link step.

A setting that needs a newer VTE degrades instead of failing:

| Setting | Needs | Behaviour on an older VTE |
| --- | --- | --- |
| `font_rendering`, `font_hinting` | VTE 0.74 (`vte_terminal_set_font_options`) | resolved with `dlsym` at start-up; when missing the toolkit defaults are used and one `font_options_unavailable` warning is logged |

Do not raise the floor without a feature that needs it: a build that only sees a
newer VTE starts using newer calls by accident, and the release binary then stops
working on the oldest supported distribution.

## The OSC 133 FIFO protocol

`OXTERM_OSC133_FIFO` points at a `0600` FIFO in the config directory
(`osc133_<pid>_<widget>.fifo`). The Bash, Zsh and Fish integration write one
event per line; Oxterm reads them for the command gutter, sticky header, history
and notifications. Broken integration can at worst produce wrong chrome; it
cannot corrupt the screen.

| Line | Meaning |
| --- | --- |
| `A` | prompt start; any payload after the `A` is ignored |
| `C<text>` | command start, `<text>` is the command line |
| `D<n>` | command end, `<n>` is the exit status (signal deaths arrive as `128+n`) |

The parser trims lines, drops blank lines and unknown leading letters, and
reassembles a line split across reads. `D` accepts `D0` and `D;0`; a missing or
non-numeric status means *no status reported*, which is shown as `status
unknown`, kept as `-1` in history, reported as JSON `null` by the control API,
and makes `oxterm wait` fail instead of guessing success. The command text is
trimmed, capped, and otherwise kept verbatim.

| Limit | Value |
| --- | --- |
| OSC 133 FIFO line buffer | 64 KiB |
| OSC 133 command text | 8 KiB |

`tests/osc133_conformance.rs` checks these numbers against the constants and
replays `tests/fixtures/osc133/session.fifo` at every read size.

## Behaviour differences and limits

- **No `B` event.** There is no command-output start marker, so a block ends at
  the cursor when `D` arrives. Programs that reposition the cursor or clear the
  screen can make a block one row too short or too long.
- **Deferred handling.** `A` and `D` are handled 30 ms after they arrive, so VTE
  has consumed the command output first; `C` is handled immediately.
- **The FIFO and the PTY can disagree.** The scripts also emit real OSC 133
  escapes through the PTY, but Oxterm reads only the FIFO. A program that prints
  its own escapes can make the grid and the recorded blocks disagree; Oxterm
  prefers the FIFO.
- **OSC 7 is display-only.** The tab tooltip and status bar show the reported
  directory, but history, notes and spawn keep using `/proc/<pid>/cwd`, which a
  program cannot forge.
- **Tab handover opens the FIFO first.** Moving a tab keeps its shell
  integration, but a shell whose last reader disappears dies of `SIGPIPE`; the
  adopting window therefore opens the source FIFO before asking the source to
  let go.
- **VTE 0.70 cannot restyle printed text.** Triggers parse `highlight_line`,
  `highlight_text` and `hyperlink`, but do not apply them yet.

## Not supported, and why

- **Desktop notifications from `OSC 9`, `OSC 777`, `OSC 99`.** Notifications come
  from OSC 133 so arbitrary output cannot trigger them. Use `oxterm action
  notify` when a program deliberately wants one.
- **Clipboard from `OSC 52`.** Copy and paste go through the GTK clipboard and a
  confirmation prompt for multi-line pastes. Whether the linked VTE also acts on
  OSC 52 depends on its version; do not rely on it in either direction.
- **xterm rectangular editing (DECCRA, DECFRA, DECERA, DECSERA, DECCARA,
  DECRARA) and the DEC locator reports.** Neither VTE nor Oxterm implements them.
- **Double-width and double-height lines (DECDHL, DECDWL), `DECSCPP`,
  `DECSNLS`.** VTE does not provide them; Oxterm does not emulate them.
- **`OSC 133;P` (prompt kind) and the richer kitty/wezterm variants.** Ignored by
  the scripts and by the parser.

## Known gaps

- A remote shell without its own OSC 7 still shows the local `ssh` client's
  directory in history entries, because only the local integration announces the
  cwd.
- Title precedence (`--title` > user-renamed tab > OSC title) has no automated
  test; the three sources race.
- Paste safety is covered by unit tests for the checked text, but not by an
  end-to-end bracketed-paste test.
- Fish scripts write through a different quoting path than Bash; there is no Fish
  session fixture.
- No test covers wide-character blocks that are scrolled, cropped or popped into
  the pager.

To add a case, append a recording to
`tests/fixtures/osc133/session.fifo`, update `expected()` in
`tests/osc133_conformance.rs`, and keep the parsing in `src/headless/osc133.rs`:
`tests/headless.rs` fails if the headless core starts depending on GTK, VTE or
the GUI modules.
