# Integrated editor pane

Status: implemented. The pane is opt-in through the Editor menu or a file jump;
no editor widget is built until it is first used.

## Goals

- Let the user edit a file without leaving the terminal session that reported it.
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
  "editor_panel_width": 420,          // 220..1200
  // Empty follows the light/dark nature of the active terminal theme.
  "editor_style_scheme": ""
}
```

The pane is shown from **Editor → Editor Panel** (`Ctrl+Shift+E`), from
**Editor → Open File in Editor...**, or automatically when a `file:line` hint is
activated. Its toolbar has Save, Undo, Redo, Find, Close document and Hide
panel buttons. `Ctrl+S` saves, `Ctrl+Z`/`Ctrl+Shift+Z` undo/redo, `Ctrl+F`
opens the search bar (Enter = next, Escape = close) and `Ctrl+W` closes the
document. **Close document** clears the buffer but keeps the pane open; if the
buffer is dirty it offers Save / Discard / Cancel. **Editor → Hide Editor
Panel** hides the pane without touching the document.

## Architecture

```
src/editor_panel.rs     GtkSourceView widget: one buffer, load/save, dirty state
src/editor_ui.rs        MainWindow panel lifecycle, focus and width persistence
src/window.rs           Paned slot, menubar entry, TerminalWindow hooks
src/terminal.rs         file:line resolution and internal/external dispatch
src/file_browser.rs     remote edit callback, unchanged mtime upload watch
src/notes.rs            note path resolution/creation without an external opener
src/headless/spec.rs    settings keys, defaults and generated documentation
```

The editor pane is a sibling of the agent and notes panes: a hidden `gtk::Box`
inside its own `gtk::Paned`. Allocating the widget tree on first use means a
window that never opens the editor pays no startup cost.

## Local files

A `file:line` hint, the terminal context menu and the file chooser all resolve
to an absolute local path before reaching the pane. `open_file_in_editor`
shows the pane, loads the file with a non-undoable initial insert and jumps to
the requested line. Saving uses `headless::persistence::atomic_write`, so a
crash cannot leave a half-written file and the 0600 permissions of private
copies are preserved.

## Notes

Notes are Markdown files in `notes_dir`, so the Notes menu opens them in this
pane instead of launching an external program. **Notes → Open Note** and
**Notes → Open Default Note** call `NotesManager::prepare_note`, which resolves
(or creates with the usual header) the file and hands it to
`open_file_in_editor`. Double-clicking or pressing Enter on a result in the
Notes panel opens the note that produced the entry; a single click still shows
the read-only preview.

Every successful save -- toolbar, `Ctrl+S`, or Save in the close-document
dialog -- fires `EditorPanel::connect_saved`, which calls
`note_refresh_panel_if_open` so the FTS index, tag list and append targets stay
in sync. Detached windows have no editor pane and keep the external
`xdg-open`/`editor_command` flow.

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

- The pane lives in the main window only. Detached windows keep the no-op
  `open_internal_file`/`editor_toggle_panel` defaults and continue to use the
  external editor.
- Only one file is open at a time; there are no editor tabs or split views.
- There is no language server, formatter, minimap or multi-cursor support.
- `editor_panel_width` is read when the pane is first built; dragging the
  handle updates it for new windows.
- `editor_style_scheme` accepts a GtkSourceView scheme id; an unknown id falls
  back to the theme-derived scheme.
- Loading a binary file is reported as an error; the editor works on UTF-8
  text only.

## Tests

- `tests/headless.rs` keeps GUI imports out of `src/headless`; the new settings
  keys are covered by the spec default and validation tests.
- `cargo run --bin oxterm-docgen -- --check` verifies that `docs/settings.md`
  and the generated completions/man page match the spec.
- GUI behaviour (panel toggle, open, save, remote upload) is exercised through
  `tests/gui_lifecycle.rs` and `tests/control_api.rs` under Xvfb.
