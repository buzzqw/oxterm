# Compatibility and terminal conformance

This document records what Oxterm promises about terminal behaviour, how each
promise is verified, and what is deliberately out of scope. It is the Oxterm
counterpart of the xterm behaviour audit the Ghostty project keeps in
[ghostty-org/ghostty#632](https://github.com/ghostty-org/ghostty/issues/632):
a claim about a terminal is only done when a test backs it up and the prose
says why.

## What Oxterm owns and what it delegates

Oxterm is a GTK3 front end over **VTE** (`vte-2.91`, through `zoha-vte`). VTE
parses the byte stream and implements CSI, DEC and OSC sequences, screen modes,
mouse reporting, selection and the character encodings. Oxterm does **not**
re-implement the parser and does not patch VTE's behaviour: re-auditing every
xterm sequence here would document VTE, not Oxterm.

That leaves a smaller layer that is genuinely Oxterm's, and that is what this
document covers:

| Layer | Owner | Evidence |
| --- | --- | --- |
| OSC 133 events over the private FIFO (prompt, command, exit status) | Oxterm | `src/headless/osc133.rs`, `tests/osc133_conformance.rs` |
| Shell integration scripts and the `ssh`/`autossh` wrappers | Oxterm | `src/terminal/shell_scripts.rs`, [manual](../manual.md#shell-integration) |
| Window and tab titles, and their precedence | Oxterm | [manual](../manual.md), `forced_title` and `tab_custom_titles` in `src/window.rs` |
| OSC 8 hyperlinks: only `http`/`https` are opened | Oxterm | `security_tests` in `src/terminal.rs` |
| OSC 7 (shell-reported directory), display only | Oxterm | `src/headless/file_uri.rs` |
| URL detection and hint mode | Oxterm | `src/terminal.rs`, `src/terminal/hints.rs` |
| Paste safety: multi-line and control-heavy pastes ask first | Oxterm | `paste_checked_text` in `src/terminal.rs` |
| Control socket (`oxterm send`, `capture`, …) | Oxterm | `tests/control_api.rs` |
| Everything else in the byte stream | VTE | CI builds and smoke-tests against VTE 0.70 (Debian 12) and the current Ubuntu release |

Because the parser is delegated, a sequence not mentioned here is neither
promised nor forbidden: it follows the VTE version the system provides. When a
user reports a sequence problem, first reproduce it in GNOME Terminal on the
same machine. If it reproduces there, the cause is most likely VTE's rather
than Oxterm's (both link the same library, though they configure it
differently), and the fix is a VTE upgrade or an upstream report — not a patch
in Oxterm.

## The oldest supported VTE is 0.70

Oxterm builds and runs against VTE 0.70, the version in Debian 12 (bookworm),
whose support runs to 2028. The `vte-0-70` CI job builds the release binary on
Debian 12 and then runs `scripts/smoke-gui.sh` under Xvfb, so the promise
covers the run time and not just the link step.

A setting that needs a newer VTE degrades instead of failing:

| Setting | Needs | Behaviour on an older VTE |
| --- | --- | --- |
| `font_rendering`, `font_hinting` | VTE 0.74 (`vte_terminal_set_font_options`) | resolved with `dlsym` at start-up; when missing the value is ignored, the toolkit defaults are used, and one `font_options_unavailable` warning is logged |

Nothing else in Oxterm depends on an API newer than 0.70. Do not raise the floor
without a feature that needs it: a build that only sees a newer VTE starts using
newer calls by accident, and the release binary then stops working on the
oldest distribution we support.


## The OSC 133 FIFO protocol

`OXTERM_OSC133_FIFO` points at a `0600` FIFO in the config directory
(`osc133_<pid>_<widget>.fifo`). The Bash, Zsh and Fish integration write one
event per line to it; Oxterm reads, parses and records the events for the
command gutter, the sticky header, the history database and the notifications.
Nothing written there reaches the screen, so a broken integration cannot corrupt
the display — it can only produce wrong chrome.

| Line | Meaning |
| --- | --- |
| `A` | prompt start; any payload after the `A` is ignored |
| `C<text>` | command start, `<text>` is the command line |
| `D<n>` | command end, `<n>` is the exit status (signal deaths arrive as `128+n`) |

The rules the parser implements, each covered by a unit test in
`src/headless/osc133.rs`:

- Lines are trimmed, so `A`, ` A ` and `A\r` are the same event.
- Blank lines and unknown leading letters are dropped. Unknown letters are
  reserved for future events, so adding one will not break older Oxterm builds.
- `D` accepts both `D0` and `D;0`. A `D` with no number (or a non-numeric one)
  means *no status reported*. That is not the same as success and is carried
  through the whole UI as unknown: the sticky header says `status unknown`, the
  prompt marker in the gutter turns grey instead of green or red, the tab keeps
  its previous failure marker, the notification says that no status was
  reported, the history database keeps `-1` for the row (the same sentinel used
  for commands typed before the integration was active), and `oxterm wait`
  fails with a message instead of reporting `0`. The control API reports the
  status as JSON `null`, never as `0`.
- The command text is everything after the `C`, trimmed, capped at the limit
  below, and kept verbatim otherwise: quotes, `;`, backslashes and wide or
  combining characters all survive, because a command line is also stored in
  the history database.
- A FIFO read may split a line or contain several; the line buffer reassembles
  them and is what the test replays at every read size.
- A line longer than the buffer is dropped, the caller logs
  `osc133_buffer_limit_reached` once, and the reader recovers on the next
  complete line.

| Limit | Value |
| --- | --- |
| OSC 133 FIFO line buffer | 64 KiB |
| OSC 133 command text | 8 KiB |

`tests/osc133_conformance.rs` checks these numbers against the constants, so
editing one without the other fails `cargo test`.

### Handover between Oxterm windows

**Tabs > Move Tab from Another Oxterm…** transfers a session, and its FIFO with
it. The order matters and is easy to get wrong:

1. the adopting window opens the source's FIFO (`pane_info` exposes the path),
2. only then does it ask the source to let go,
3. the source stops its watch, turns off the local view, closes its end and
   closes the tab; the FIFO file is preserved,
4. the adopting tab installs the fd it already holds and reads the events that
   queued in between.

A shell whose last reader disappears dies of SIGPIPE at its next prompt (exit
status 141), so opening the FIFO after the handover would kill the session the
move is supposed to save. For the milliseconds between steps 1 and 3 both ends
are readers, so an event produced exactly then can be consumed by the window
that is going away; at worst one command-block boundary is missed. The adopted
tab runs `oxterm -a <session>` as a relay: it is a broker client, not a local
shell, and closing it ends the session.

Destroying the source tab must not signal the child either: VTE signals the
child it watched when its widget is disposed, so Oxterm reaps children with its
own child watch instead of `vte_terminal_watch_child`.

### Behaviour that differs from a generic OSC 133 consumer

- **No `B` event.** The integration reports no command-output start, so a block
  ends where the cursor is when `D` arrives. Programs that position the cursor
  after their last line (or clear the screen) can therefore make a block one
  row too short or too long.
- **Deferred handling.** `A` and `D` are handled 30 ms after they arrive, on
  purpose: VTE may not have consumed the command's output yet, and handling
  them immediately put prompts on the command's first row. `C` is handled
  immediately because its row comes from the pending keystroke.
- **The FIFO and the PTY can disagree.** The scripts also emit real
  `OSC 133` escapes (`ESC ] 133 ; … BEL`) through the PTY, which VTE consumes;
  the FIFO is the only channel Oxterm reads. A program that prints its own
  escapes, or a full-screen application that clears the screen, can make the
  terminal grid and the recorded blocks disagree. Oxterm prefers the FIFO.
- **`/proc` decides where commands run; OSC 7 only describes.** The scripts
  emit `OSC 7;file://<cwd>` and VTE stores it. `get_cwd()` still reads
  `/proc/<pid>/cwd` for history entries, notes and the directory of a new tab,
  because any program can print an OSC 7 it likes while `/proc` cannot lie
  about the process. The reported URI is decoded by `src/headless/file_uri.rs`
  and shown in the tab tooltip (and at the left of the status bar when it
  differs from `/proc`, which is what a remote shell reports). The decoder only
  accepts a local `file://` URI, drops query and fragment, removes control
  characters and caps the result, so a hostile OSC 7 can at worst produce an
  ugly tooltip.

## Not supported, and why

- **Desktop notifications from `OSC 9`, `OSC 777`, `OSC 99`.** Not implemented;
  notifications come from OSC 133 (command finished, exit status, duration) so
  that they cannot be triggered by arbitrary output. A program that wants a
  notification can use `oxterm notify` on the control socket.
- **Clipboard from `OSC 52`.** Oxterm never reads or writes the clipboard from
  terminal output; copy and paste go through the GTK clipboard and a
  confirmation prompt for multi-line pastes. Whether the linked VTE also acts on
  OSC 52 depends on its version — do not rely on it in either direction.
- **xterm rectangular editing (DECCRA, DECFRA, DECERA, DECSERA, DECCARA,
  DECRARA) and the DEC locator reports.** Neither VTE nor Oxterm implements
  them; Oxterm does not add them on top.
- **Double-width and double-height lines (DECDHL, DECDWL), `DECSCPP`,
  `DECSNLS`.** Rendering-level features VTE does not provide; Oxterm does not
  emulate them.
- **`OSC 133;P` (prompt kind) and the richer kitty/wezterm variants.** Ignored
  by the scripts and by the parser.

## Checklist for the next audit

In the spirit of the upstream issue, open items are listed here with the reason
they matter. A checked item has a test and prose; a ⚠️ item is a known
divergence without one.

- ✔ **A `D` with no status is no longer success.** `last_exit_code()` is an
  `Option`, the sticky header shows `status unknown`, the tab mark is left
  untouched and `oxterm wait` fails loudly. Covered by
  `sticky_header_admits_an_unknown_status`,
  `only_a_reported_status_moves_the_tab_failure_mark` and
  `an_unknown_status_is_neither_success_nor_failure`.
- ✔ **A repeated `A` no longer adds a second prompt marker on the same row.**
  The integration sends `A` when it loads and again from the first `precmd`,
  which used to make prompt navigation stop twice on one line. Covered by
  `repeated_prompt_markers_on_one_row_are_collapsed`.
- ✔ **Moving a tab keeps its shell integration.** The adopting window holds
  the source FIFO across the handover and the moved shell keeps delivering
  events. Covered by `handover_keeps_the_moved_session_alive` (GUI suite) and
  the broker move/attach tests.
- ✔ **OSC 7 is decoded and shown, but never trusted.** The tab tooltip and the
  status bar show the shell-reported directory; history, notes and spawn keep
  using `/proc`. Covered by the `file_uri` unit tests (host, scheme, escapes,
  control characters, length cap).
- ⚠️ **A remote shell without its own OSC 7 still shows the local `ssh`
  client's directory** in history entries, because only the local integration
  script announces the cwd. Fixing that needs the remote side to speak OSC 7,
  which Oxterm cannot install there.
- ☐ **Title precedence** (`--title` > user-renamed tab > OSC title) has no
  automated test; the three sources race and only `manual.md` describes them.
- ☐ **Paste safety** is covered by unit tests for the checked text, but not by
  an end-to-end test through the widget for bracketed paste.
- ☐ **A Fish session fixture.** The Fish scripts write through a different
  quoting path than Bash; one recording per shell would catch divergence
  between the two scripts.
- ☐ **Wide-character blocks.** No test covers a command whose output contains
  emoji or CJK text and that is then scrolled, cropped or popped into the
  pager. This is the same class of bug the upstream audit flags for
  insert/delete line: multi-cell characters must be split at every operation
  that cuts text.

## Adding a case

1. Record the FIFO traffic of the situation (or write it by hand) and append it
   to `tests/fixtures/osc133/session.fifo`, with a comment saying what it
   reproduces.
2. Add the expected events to `expected()` in `tests/osc133_conformance.rs`.
   The test replays the file at several read sizes, so a newline in the wrong
   place is caught.
3. If the rule is new, state it in the protocol section above and add a unit
   test in `src/headless/osc133.rs`.
4. If the change touches the reader in `src/terminal.rs`, keep the parsing in
   `src/headless/osc133.rs`: `tests/headless.rs` fails if the headless core
   starts depending on GTK, VTE or the GUI modules.
