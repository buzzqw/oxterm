# Notes/Editor dock

Status: implemented. The Notes panel and the integrated editor share one
right-hand dock with two tabs. The widget tree is built on first use, like the
other optional panes.

## Goals

- Let the user edit a file without leaving the terminal session that reported it.
- Give notes and files one place on screen: switching a tab replaces opening a
  second dock, and a note has a single editable view.
- Keep the editor inside the same process, with the same settings, themes,
  keybindings and control surface as the terminal.
- Reuse GtkSourceView for the solved parts: syntax highlighting, undo/redo,
  line numbers, auto-indent and bracket matching.
- Make remote editing work through the connection the user already has open,
  not through a second login in a separate application.

Non-goals: becoming a full IDE, embedding an LSP client, a build system, LaTeX
tooling or a minimap. Those remain the job of a dedicated editor such as
NotePadPQ, which `editor_command` can still launch.

## Enabling

```jsonc
{
  // Open file:line locations from hints and the context menu inside Oxterm.
  // Set to false to keep the old ${EDITOR} behaviour.
  "editor_use_internal": true,
  "editor_panel_width": 420,          // width of the shared dock, 220..1200
  // Empty follows the light/dark nature of the active terminal theme.
  "editor_style_scheme": ""
}
```

The dock is shown by the single **Workspace** button in the menu bar
(`Ctrl+Shift+E`), or automatically for a `file:line` hint. The button is a
plain toggle: there is no Notes or Editor menu to duplicate the panel actions.
The Close/Hide buttons inside the panels hide the dock without touching the
editor document.

The editor toolbar has Open, Save, Undo, Redo, Find, Close document and Hide
buttons. Open uses the same file chooser as the context-menu action.
`Ctrl+S` saves, `Ctrl+Z`/`Ctrl+Shift+Z` undo/redo, `Ctrl+F` opens the search bar
(Enter = next, Escape = close) and `Ctrl+W` closes the document. **Close
document** clears the buffer but keeps the dock open; if the buffer is dirty it
offers Save / Discard / Cancel.

## Architecture

```
src/workspace_ui.rs     one Paned slot, Notebook tabs, show/hide, width persistence
src/editor_panel.rs     GtkSourceView widget: one buffer, load/save, dirty state
src/editor_ui.rs        editor-only actions: open, save, file chooser
src/notes_panel.rs      Notes tab: search, tag filter, snippet list, append box
src/notes_ui.rs         notes-only actions: index, search, append
src/window.rs           Paned slot, menubar entries, TerminalWindow hooks
src/terminal.rs         file:line resolution and internal/external dispatch
src/file_browser.rs     remote edit callback, unchanged mtime upload watch
src/notes.rs            note path resolution/creation without an external opener
src/headless/spec.rs    settings keys, defaults and generated documentation
```

The dock is a hidden `gtk::Box` inside a single `gtk::Paned`; a `gtk::Notebook`
owns the two panels. Allocating the tree on first use means a window that never
opens either feature pays no startup cost. Actions switch the active tab instead
of creating a second dock.

## Notes

Notes are Markdown files in `notes_dir`, so they open in the Editor tab
instead of launching an external program. Double-clicking a search result, or
`/onotes` at the prompt, calls `NotesManager::prepare_note`, which resolves (or
creates with the usual header) the file and hands it to `open_file_in_editor`.
The Notes tab header keeps **New** and **Folder** for creating a note and
opening the notes directory; the terminal context menu keeps the actions that
operate on a selection, a command output or a file.

The Notes tab has no read-only preview: a result shows its FTS snippet, a single
click only preselects the append target, and double-click or Enter opens that
note in the Editor tab.

Every successful save -- toolbar, `Ctrl+S`, or Save in the close-document
dialog -- fires `EditorPanel::connect_saved`, which calls
`note_refresh_panel_if_open` so the FTS index, tag list and append targets stay
in sync when the Notes tab is visible. Detached windows have no dock and keep
the external `xdg-open`/`editor_command` flow.

## Local files

A `file:line` hint, the terminal context menu and the file chooser all resolve
to an absolute local path before reaching the editor. `open_file_in_editor`
shows the dock on the Editor tab, loads the file with a non-undoable initial
insert and jumps to the requested line. Saving uses
`headless::persistence::atomic_write`, so a crash cannot leave a half-written
file and the 0600 permissions of private copies are preserved.

## Remote files

The file browser already downloads a remote file into a private cache copy and
watches its mtime every two seconds to upload changes. The browser used to call
`xdg-open` on that copy; it now calls back into the main window through a
closure supplied by `connections_ui.rs`. The watch is editor-agnostic, so the
internal editor, an external editor and atomic saves all produce the same
upload.

## Themes and typography

GtkSourceView style schemes are selected from `editor_style_scheme` when set.
Otherwise the foreground/background of the active terminal theme is classified
with `headless::themes::is_dark_color` and mapped to a dark scheme
(`oblivion`, `solarized-dark`, `cobalt`) or a light one (`classic`, `tango`,
`solarized-light`). No theme file format changes are needed. The view uses the
configured terminal font family and size through a CSS provider.

## Limitations

- The dock lives in the main window only. Detached windows keep the no-op
  `open_internal_file`/`editor_toggle_panel` defaults and continue to use the
  external editor.
- Only one editor document is open at a time; there are no editor tabs or split
  views inside the Editor tab.
- There is no language server, formatter, minimap or multi-cursor support.
- `editor_panel_width` is applied when the dock is first shown; dragging the
  handle updates it for subsequent windows.
- `editor_style_scheme` accepts a GtkSourceView scheme id; an unknown id falls
  back to the theme-derived scheme.
- Loading a binary file is reported as an error; the editor works on UTF-8
  text only.

## Tests

- `tests/headless.rs` keeps GUI imports out of `src/headless`; the settings keys
  are covered by the spec default and validation tests.
- `cargo run --bin oxterm-docgen -- --check` verifies that `docs/settings.md`
  and the generated completions/man page match the spec.
- GUI behaviour (tab switching, open, save, remote upload) is exercised through
  `tests/gui_lifecycle.rs` and `tests/control_api.rs` under Xvfb.
