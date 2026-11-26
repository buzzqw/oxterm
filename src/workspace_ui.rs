//! The single right-hand dock that hosts both the Notes and the Editor tabs.
//!
//! Keeping them in one `gtk::Paned` (and one width setting) avoids two competing
//! docks: an action switches the active tab instead of opening a second panel.
//! The widget tree is built on first use, like the other optional panes.

use glib::subclass::prelude::*;
use gtk::prelude::*;

use crate::settings::settings;
use crate::window::{MainWindow, TerminalWindow};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceTab {
    Notes,
    Editor,
    /// Third tab: remote resource monitor above the remote log viewer.
    Monitor,
}

impl MainWindow {
    pub(crate) fn ensure_workspace(&self) -> bool {
        let imp = self.imp();
        if imp.workspace_notebook.borrow().is_some() {
            return true;
        }
        let Some(slot) = imp.workspace_slot.borrow().clone() else {
            return false;
        };

        let notebook = gtk::Notebook::new();
        notebook.set_scrollable(true);

        let notes_panel = crate::notes_panel::NotesPanel::build();
        {
            let weak = crate::SendWeak::new(self);
            notes_panel.close.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.set_workspace_visible(false);
                }
            });
            let weak = crate::SendWeak::new(self);
            notes_panel.refresh.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.reload_notes_panel();
                }
            });
            // The two actions that used to live in the Notes menu are now in
            // the Notes tab itself: one interface for notes and text files.
            let weak = crate::SendWeak::new(self);
            notes_panel.new_note.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.note_new();
                }
            });
            let weak = crate::SendWeak::new(self);
            notes_panel.folder.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.note_open_folder();
                }
            });
            let weak = crate::SendWeak::new(self);
            notes_panel.search.connect_search_changed(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.schedule_notes_search();
                }
            });
            let weak = crate::SendWeak::new(self);
            notes_panel.tag.connect_changed(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.schedule_notes_search();
                }
            });
            let weak = crate::SendWeak::new(self);
            notes_panel.list.connect_row_selected(move |_, row| {
                if let (Some(w), Some(row)) = (weak.upgrade(), row) {
                    w.notes_panel_select(row.index());
                }
            });
            // A result is opened in the Editor tab; the list itself keeps only
            // the snippet, so there is no second read-only view of the note.
            let weak = crate::SendWeak::new(self);
            notes_panel.list.connect_row_activated(move |_, row| {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let path = window
                    .imp()
                    .notes_panel
                    .borrow()
                    .as_ref()
                    .and_then(|panel| panel.note_path_at(row.index()));
                if let Some(path) = path {
                    window.open_file_in_editor(&path, None, None);
                }
            });
            let weak = crate::SendWeak::new(self);
            notes_panel.append.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.notes_panel_append();
                }
            });
            let weak = crate::SendWeak::new(self);
            notes_panel.entry.connect_activate(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.notes_panel_append();
                }
            });
        }

        let editor_panel = crate::editor_panel::EditorPanel::build();
        {
            let weak = crate::SendWeak::new(self);
            editor_panel.open_file.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.choose_file_in_editor();
                }
            });
            let weak = crate::SendWeak::new(self);
            editor_panel.hide_panel.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.set_workspace_visible(false);
                }
            });
            // Saving a note changes its indexed text. Refresh through the
            // window so Ctrl+S and the Save button behave like menu saves.
            let weak = crate::SendWeak::new(self);
            editor_panel.connect_saved(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_refresh_panel_if_open();
                }
            });
        }

        // The monitoring features share the third tab instead of opening a
        // second right-hand dock (which was the source of layout confusion).
        let monitor_target = self.monitor_target_from_current();
        let monitor_panel = crate::monitor_panel::MonitorPanel::build();
        monitor_panel.set_target(monitor_target.clone());
        {
            let weak = crate::SendWeak::new(self);
            monitor_panel.close.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.set_workspace_visible(false);
                }
            });
        }

        let log_viewer_panel = crate::log_viewer_panel::LogViewerPanel::build();
        log_viewer_panel.set_target(monitor_target);
        {
            let weak = crate::SendWeak::new(self);
            log_viewer_panel.close.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.set_workspace_visible(false);
                }
            });
        }

        let monitor_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        monitor_page.pack_start(&monitor_panel.root, false, false, 0);
        monitor_page.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            0,
        );
        monitor_page.pack_start(&log_viewer_panel.root, true, true, 0);

        notebook.append_page(&notes_panel.root, Some(&gtk::Label::new(Some("Notes"))));
        notebook.append_page(&editor_panel.root, Some(&gtk::Label::new(Some("Editor"))));
        notebook.append_page(&monitor_page, Some(&gtk::Label::new(Some("Monitor"))));
        // All roots carry `no_show_all` from their standalone past. The
        // notebook is the only visibility gate now, so make the pages showable.
        for root in [
            &notes_panel.root,
            &editor_panel.root,
            &monitor_panel.root,
            &log_viewer_panel.root,
        ] {
            root.set_no_show_all(false);
        }

        let weak = crate::SendWeak::new(self);
        notebook.connect_switch_page(move |_, _, page| {
            if page == 0 {
                if let Some(window) = weak.upgrade() {
                    window.reload_notes_panel();
                }
            }
        });
        slot.pack_start(&notebook, true, true, 0);
        // Store the panels before showing the tree: a page switch can fire
        // synchronously and its handler reloads through the window.
        *imp.notes_panel.borrow_mut() = Some(notes_panel);
        *imp.editor_panel.borrow_mut() = Some(editor_panel);
        *imp.monitor_panel.borrow_mut() = Some(monitor_panel);
        *imp.log_viewer_panel.borrow_mut() = Some(log_viewer_panel);
        // `show()` on the slot is not recursive: the notebook needs its own
        // explicit `show_all()` or the dock would be an empty box.
        notebook.show_all();
        if let Some(pane) = slot
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Paned>().ok())
        {
            let weak = crate::SendWeak::new(self);
            pane.connect_notify_local(Some("position"), move |pane, _| {
                if let Some(window) = weak.upgrade() {
                    window.remember_workspace_width(pane);
                }
            });
        }
        *imp.workspace_notebook.borrow_mut() = Some(notebook);
        true
    }

    pub(crate) fn workspace_visible(&self) -> bool {
        self.imp()
            .workspace_slot
            .borrow()
            .as_ref()
            .map(|slot| slot.is_visible())
            .unwrap_or(false)
    }

    pub(crate) fn set_workspace_visible(&self, visible: bool) {
        if visible && !self.ensure_workspace() {
            return;
        }
        if visible {
            let was_visible = self.workspace_visible();
            if let Some(slot) = self.imp().workspace_slot.borrow().clone() {
                slot.show();
            }
            // Position only when opening the dock: switching tabs must not undo
            // a width the user just dragged. The allocation needs one main-loop
            // pass after `show()`.
            if !was_visible {
                let weak = self.downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(window) = weak.upgrade() {
                        window.position_workspace_dock();
                    }
                });
            }
        } else {
            // Stop remote polling while the dock is hidden.
            if let Some(panel) = self.imp().monitor_panel.borrow().as_ref() {
                panel.pause();
            }
            if let Some(panel) = self.imp().log_viewer_panel.borrow().as_ref() {
                panel.pause();
            }
            if let Some(slot) = self.imp().workspace_slot.borrow().clone() {
                slot.hide();
            }
            if let Some(terminal) = self.current_terminal() {
                terminal.vte().grab_focus();
            }
        }
    }

    fn position_workspace_dock(&self) {
        let Some(pane) = self
            .imp()
            .workspace_slot
            .borrow()
            .as_ref()
            .and_then(|slot| slot.parent())
            .and_then(|parent| parent.downcast::<gtk::Paned>().ok())
        else {
            return;
        };
        let width = settings().get_i64("editor_panel_width").clamp(220, 1200) as i32;
        let extent = pane.allocated_width();
        if extent <= width {
            return;
        }
        self.imp().workspace_positioning.set(true);
        pane.set_position(extent - width);
        self.imp().workspace_positioning.set(false);
    }

    fn remember_workspace_width(&self, pane: &gtk::Paned) {
        if !self.workspace_visible() || self.imp().workspace_positioning.get() {
            return;
        }
        let extent = pane.allocated_width();
        if extent <= 1 {
            return;
        }
        let width = ((extent - pane.position()) as i64).clamp(220, 1200);
        if (width - settings().get_i64("editor_panel_width")).abs() <= 1 {
            return;
        }
        let generation = self.imp().workspace_width_generation.get().wrapping_add(1);
        self.imp().workspace_width_generation.set(generation);
        let weak = self.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if window.imp().workspace_width_generation.get() != generation {
                return;
            }
            settings().set_i64("editor_panel_width", width);
        });
    }

    pub(crate) fn notes_tab_active(&self) -> bool {
        self.imp()
            .workspace_notebook
            .borrow()
            .as_ref()
            .and_then(|notebook| notebook.current_page())
            .map(|page| page == 0)
            .unwrap_or(false)
    }

    pub(crate) fn show_workspace_tab(&self, tab: WorkspaceTab) {
        self.set_workspace_visible(true);
        if let Some(notebook) = self.imp().workspace_notebook.borrow().clone() {
            let page = match tab {
                WorkspaceTab::Notes => 0,
                WorkspaceTab::Editor => 1,
                WorkspaceTab::Monitor => 2,
            };
            notebook.set_current_page(Some(page));
        }
    }

    pub(crate) fn toggle_workspace_tab(&self, tab: WorkspaceTab) {
        let page = match tab {
            WorkspaceTab::Notes => 0,
            WorkspaceTab::Editor => 1,
            WorkspaceTab::Monitor => 2,
        };
        let active = self.workspace_visible()
            && self
                .imp()
                .workspace_notebook
                .borrow()
                .as_ref()
                .and_then(|notebook| notebook.current_page())
                == Some(page);
        if active {
            self.set_workspace_visible(false);
        } else {
            self.show_workspace_tab(tab);
        }
    }

    /// The single menu-bar action: show or hide the shared dock, keeping the
    /// tab that was last active.
    pub(crate) fn toggle_workspace(&self) {
        self.set_workspace_visible(!self.workspace_visible());
    }
}
