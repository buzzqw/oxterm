//! Editor-specific actions for the shared right-hand dock. Panel construction,
//! tab switching and width persistence live in `workspace_ui.rs`.

use std::path::Path;

use glib::subclass::prelude::*;
use gtk::prelude::*;

use crate::window::{MainWindow, TerminalWindow};
use crate::workspace_ui::WorkspaceTab;

impl MainWindow {
    /// Open a resolved local path in the Editor tab, optionally jumping to a line.
    pub(crate) fn open_file_in_editor(&self, path: &Path, line: Option<u32>, column: Option<u32>) {
        self.show_workspace_tab(WorkspaceTab::Editor);
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

    /// Hide the dock without touching the document (menu action).
    pub(crate) fn hide_workspace(&self) {
        self.set_workspace_visible(false);
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
