//! Window-side wiring for the integrated editor pane. Mirrors the lazy-panel
//! lifecycle used by the notes panel: nothing is built until it is first shown.

use std::path::Path;

use glib::subclass::prelude::*;
use gtk::prelude::*;

use crate::settings::settings;
use crate::window::{MainWindow, TerminalWindow};

impl MainWindow {
    fn ensure_editor_panel(&self) -> bool {
        if self.imp().editor_panel.borrow().is_some() {
            return true;
        }
        let Some(slot) = self.imp().editor_slot.borrow().clone() else {
            return false;
        };
        let panel = crate::editor_panel::EditorPanel::build();
        {
            let weak = crate::SendWeak::new(self);
            panel.close.connect_clicked(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.set_editor_panel_visible(false);
                }
            });
            let weak = crate::SendWeak::new(self);
            panel.save.connect_clicked(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.save_editor_file();
                }
            });
        }
        slot.pack_start(&panel.root, true, true, 0);
        slot.show();
        if let Some(pane) = slot
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Paned>().ok())
        {
            let width = settings().get_i64("editor_panel_width").clamp(220, 1200) as i32;
            let extent = pane.allocated_width();
            if extent > width {
                self.imp().editor_positioning.set(true);
                pane.set_position(extent - width);
                self.imp().editor_positioning.set(false);
            }
            let weak = crate::SendWeak::new(self);
            pane.connect_notify_local(Some("position"), move |pane, _| {
                if let Some(window) = weak.upgrade() {
                    window.remember_editor_panel_width(pane);
                }
            });
        }
        *self.imp().editor_panel.borrow_mut() = Some(panel);
        true
    }

    fn remember_editor_panel_width(&self, pane: &gtk::Paned) {
        if !self.editor_panel_visible() || self.imp().editor_positioning.get() {
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
        let generation = self.imp().editor_width_generation.get().wrapping_add(1);
        self.imp().editor_width_generation.set(generation);
        let weak = self.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if window.imp().editor_width_generation.get() != generation {
                return;
            }
            settings().set_i64("editor_panel_width", width);
        });
    }

    pub(crate) fn editor_panel_visible(&self) -> bool {
        self.imp()
            .editor_panel
            .borrow()
            .as_ref()
            .map(|panel| panel.root.is_visible())
            .unwrap_or(false)
    }

    pub(crate) fn set_editor_panel_visible(&self, visible: bool) {
        if visible && !self.ensure_editor_panel() {
            return;
        }
        if let Some(panel) = self.imp().editor_panel.borrow().as_ref() {
            if visible {
                if let Some(slot) = self.imp().editor_slot.borrow().clone() {
                    slot.show();
                }
                // The panel carries `no_show_all` so the window's own
                // `show_all()` keeps it hidden until it is asked for.
                panel.root.set_no_show_all(false);
                panel.root.show_all();
                panel.root.set_no_show_all(true);
            } else {
                panel.root.hide();
                if let Some(slot) = self.imp().editor_slot.borrow().clone() {
                    slot.hide();
                }
                if let Some(terminal) = self.current_terminal() {
                    terminal.vte().grab_focus();
                }
            }
        }
    }

    pub(crate) fn toggle_editor_panel(&self) {
        let visible = self.editor_panel_visible();
        self.set_editor_panel_visible(!visible);
    }

    /// Open a resolved local path in the pane, optionally jumping to a line.
    pub(crate) fn open_file_in_editor(&self, path: &Path, line: Option<u32>, column: Option<u32>) {
        self.set_editor_panel_visible(true);
        let Some(panel) = self.imp().editor_panel.borrow().clone() else {
            return;
        };
        if let Err(error) = panel.open(path, line) {
            self.note_feedback(
                &format!("Cannot open {} in the editor: {}", path.display(), error),
                true,
            );
        } else if let Some(column) = column.filter(|column| *column > 1) {
            panel.set_status(&format!("{} · column {}", path.display(), column));
        }
    }

    pub(crate) fn save_editor_file(&self) {
        let Some(panel) = self.imp().editor_panel.borrow().clone() else {
            return;
        };
        match panel.save() {
            Ok(_) => self.note_feedback("Editor file saved", false),
            Err(error) => self.note_feedback(&format!("Cannot save editor file: {}", error), true),
        }
    }

    pub(crate) fn close_editor_panel(&self) {
        self.set_editor_panel_visible(false);
    }

    pub(crate) fn choose_file_in_editor(&self) {
        let dialog = gtk::FileChooserDialog::with_buttons(
            Some("Open file in editor"),
            Some(self),
            gtk::FileChooserAction::Open,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Open", gtk::ResponseType::Accept),
            ],
        );
        dialog.set_modal(true);
        let weak = crate::SendWeak::new(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let (Some(window), Some(path)) = (weak.upgrade(), dialog.filename()) {
                    window.open_file_in_editor(&path, None, None);
                }
            }
            dialog.close();
        });
        dialog.show();
    }
}
