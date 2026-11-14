//! GTK4 and VTE4 compatibility layer for Oxterm.
//!
//! Provides adapter traits and types that bridge GTK3 idioms
//! (menus, containers, dialogs, event handling) to modern GTK4 APIs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use gtk::gdk;
pub use gtk::glib;
pub use gtk::prelude::*;

pub use gtk::glib_compat;
pub use gtk::keys;
pub use gtk::TreeViewColumnExt;

pub use zoha_vte;
pub use zoha_vte as vte;
pub use zoha_vte_sys;
pub use zoha_vte_sys as vte_sys;

/// Clipboard wrapper for GTK4.
pub struct Clipboard;

impl Clipboard {
    pub fn get(_atom: &str) -> Self {
        Clipboard
    }

    pub fn set_text(&self, text: &str) {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(text);
        }
    }

    pub fn request_text<F: FnOnce(Option<&str>) + 'static>(&self, callback: F) {
        if let Some(display) = gdk::Display::default() {
            let clipboard = display.clipboard();
            clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
                if let Ok(Some(gstr)) = result {
                    callback(Some(gstr.as_str()));
                } else {
                    callback(None);
                }
            });
        } else {
            callback(None);
        }
    }
}

pub const SELECTION_CLIPBOARD: &str = "CLIPBOARD";
pub const SELECTION_PRIMARY: &str = "PRIMARY";

/// Compatibility wrapper for MenuItem.
#[derive(Clone)]
pub struct MenuItem {
    pub label: String,
    pub action: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    pub submenu: Rc<RefCell<Option<Menu>>>,
    pub sensitive: Rc<Cell<bool>>,
    pub tooltip: Rc<RefCell<Option<String>>>,
    pub custom_widget: Rc<RefCell<Option<gtk::Widget>>>,
}

impl MenuItem {
    pub fn new() -> Self {
        Self::with_label("")
    }

    pub fn with_label(label: &str) -> Self {
        Self {
            label: label.to_string(),
            action: Rc::new(RefCell::new(None)),
            submenu: Rc::new(RefCell::new(None)),
            sensitive: Rc::new(Cell::new(true)),
            tooltip: Rc::new(RefCell::new(None)),
            custom_widget: Rc::new(RefCell::new(None)),
        }
    }

    pub fn connect_activate<F: Fn(&MenuItem) + 'static>(&self, f: F) {
        let this = self.clone();
        *self.action.borrow_mut() = Some(Box::new(move || f(&this)));
    }

    pub fn set_submenu(&self, submenu: Option<&Menu>) {
        *self.submenu.borrow_mut() = submenu.cloned();
    }

    pub fn set_sensitive(&self, sensitive: bool) {
        self.sensitive.set(sensitive);
    }

    pub fn set_tooltip_text(&self, tip: Option<&str>) {
        *self.tooltip.borrow_mut() = tip.map(String::from);
    }

    pub fn add<W: IsA<gtk::Widget>>(&self, widget: &W) {
        *self.custom_widget.borrow_mut() = Some(widget.clone().upcast());
    }

    pub fn destroy(&self) {}
}

/// Compatibility wrapper for Menu.
#[derive(Clone, Default)]
pub struct Menu {
    pub items: Rc<RefCell<Vec<MenuItem>>>,
}

impl Menu {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&self, item: &MenuItem) {
        self.items.borrow_mut().push(item.clone());
    }

    pub fn show_all(&self) {}

    pub fn destroy(&self) {}

    pub fn connect_deactivate<F: Fn(&Menu) + 'static>(&self, _f: F) {}

    pub fn popup_at_pointer(&self, _event: Option<&EventButton>) {
        // GTK4 popover creation at active focus/pointer
    }

    pub fn to_popover(&self) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.add_css_class("oxterm-tab-menu");
        for item in self.items.borrow().iter() {
            if let Some(custom) = item.custom_widget.borrow().as_ref() {
                vbox.append(custom);
                continue;
            }
            let btn = gtk::Button::with_label(&item.label);
            btn.set_has_frame(false);
            btn.set_sensitive(item.sensitive.get());
            if let Some(tip) = item.tooltip.borrow().as_ref() {
                btn.set_tooltip_text(Some(tip));
            }
            let action = item.action.clone();
            let pop_clone = popover.clone();
            btn.connect_clicked(move |_| {
                pop_clone.popdown();
                if let Some(act) = action.borrow().as_ref() {
                    act();
                }
            });
            vbox.append(&btn);
        }
        popover.set_child(Some(&vbox));
        popover
    }
}

/// Key event data compatible with GTK3 handlers.
#[derive(Clone, Debug)]
pub struct EventKey {
    pub keyval: gdk::Key,
    pub state: gdk::ModifierType,
}

impl EventKey {
    pub fn keyval(&self) -> gdk::Key {
        self.keyval
    }

    pub fn state(&self) -> gdk::ModifierType {
        self.state
    }
}

/// Mouse button event data compatible with GTK3 handlers.
#[derive(Clone, Debug)]
pub struct EventButton {
    pub button: u32,
    pub n_press: i32,
    pub x: f64,
    pub y: f64,
    pub state: gdk::ModifierType,
}

impl EventButton {
    pub fn button(&self) -> u32 {
        self.button
    }

    pub fn position(&self) -> (f64, f64) {
        (self.x, self.y)
    }

    pub fn event_type(&self) -> gdk::EventType {
        if self.n_press >= 2 {
            gdk::EventType::DoubleButtonPress
        } else {
            gdk::EventType::ButtonPress
        }
    }

    pub fn state(&self) -> gdk::ModifierType {
        self.state
    }
}

/// Extension trait for Box compatibility.
pub trait CompatBoxExt {
    fn pack_start<W: IsA<gtk::Widget>>(&self, child: &W, expand: bool, fill: bool, padding: u32);
    fn pack_end<W: IsA<gtk::Widget>>(&self, child: &W, expand: bool, fill: bool, padding: u32);
}

impl CompatBoxExt for gtk::Box {
    fn pack_start<W: IsA<gtk::Widget>>(&self, child: &W, expand: bool, _fill: bool, _padding: u32) {
        if expand {
            child.set_hexpand(true);
            child.set_vexpand(true);
        }
        self.append(child);
    }

    fn pack_end<W: IsA<gtk::Widget>>(&self, child: &W, expand: bool, _fill: bool, _padding: u32) {
        if expand {
            child.set_hexpand(true);
            child.set_vexpand(true);
        }
        self.prepend(child);
    }
}

/// Extension trait for Paned compatibility.
pub trait CompatPanedExt {
    fn pack1<W: IsA<gtk::Widget>>(&self, child: &W, resize: bool, shrink: bool);
    fn pack2<W: IsA<gtk::Widget>>(&self, child: &W, resize: bool, shrink: bool);
}

impl CompatPanedExt for gtk::Paned {
    fn pack1<W: IsA<gtk::Widget>>(&self, child: &W, resize: bool, shrink: bool) {
        self.set_start_child(Some(child));
        self.set_resize_start_child(resize);
        self.set_shrink_start_child(shrink);
    }

    fn pack2<W: IsA<gtk::Widget>>(&self, child: &W, resize: bool, shrink: bool) {
        self.set_end_child(Some(child));
        self.set_resize_end_child(resize);
        self.set_shrink_end_child(shrink);
    }
}

/// Extension trait for single-child container compatibility.
pub trait CompatBinExt {
    fn add<W: IsA<gtk::Widget>>(&self, child: &W);
}

impl CompatBinExt for gtk::ScrolledWindow {
    fn add<W: IsA<gtk::Widget>>(&self, child: &W) {
        self.set_child(Some(child));
    }
}

impl CompatBinExt for gtk::Window {
    fn add<W: IsA<gtk::Widget>>(&self, child: &W) {
        self.set_child(Some(child));
    }
}

impl CompatBinExt for gtk::ApplicationWindow {
    fn add<W: IsA<gtk::Widget>>(&self, child: &W) {
        self.set_child(Some(child));
    }
}

impl CompatBinExt for gtk::Frame {
    fn add<W: IsA<gtk::Widget>>(&self, child: &W) {
        self.set_child(Some(child));
    }
}

impl CompatBinExt for gtk::Revealer {
    fn add<W: IsA<gtk::Widget>>(&self, child: &W) {
        self.set_child(Some(child));
    }
}

impl CompatBinExt for gtk::Button {
    fn add<W: IsA<gtk::Widget>>(&self, child: &W) {
        self.set_child(Some(child));
    }
}

impl CompatBinExt for gtk::Overlay {
    fn add<W: IsA<gtk::Widget>>(&self, child: &W) {
        self.set_child(Some(child));
    }
}

/// Extension trait for general widget compatibility.
pub trait CompatWidgetExt: IsA<gtk::Widget> {
    fn show_all(&self) {
        self.set_visible(true);
    }

    fn set_no_show_all(&self, _no_show_all: bool) {}

    fn style_context(&self) -> CompatStyleContext<Self> {
        CompatStyleContext { widget: self }
    }
}

impl<T: IsA<gtk::Widget>> CompatWidgetExt for T {}

pub struct CompatStyleContext<'a, W: ?Sized> {
    widget: &'a W,
}

impl<'a, W: IsA<gtk::Widget>> CompatStyleContext<'a, W> {
    pub fn add_class(&self, class: &str) {
        self.widget.add_css_class(class);
    }

    pub fn remove_class(&self, class: &str) {
        self.widget.remove_css_class(class);
    }

    pub fn lookup_color(&self, _name: &str) -> Option<gdk::RGBA> {
        Some(gdk::RGBA::new(1.0, 1.0, 1.0, 1.0))
    }

    pub fn color(&self, _flags: gtk::StateFlags) -> gdk::RGBA {
        gdk::RGBA::new(1.0, 1.0, 1.0, 1.0)
    }
}

/// Extension trait for synchronous Dialog::run on GTK4.
pub trait CompatDialogExt {
    fn run(&self) -> gtk::ResponseType;
}

impl<T: IsA<gtk::Dialog>> CompatDialogExt for T {
    fn run(&self) -> gtk::ResponseType {
        let loop_ = glib::MainLoop::new(None, false);
        let resp_cell = Rc::new(Cell::new(gtk::ResponseType::None));
        let loop_clone = loop_.clone();
        let resp_clone = resp_cell.clone();
        let handler_id = self.connect_response(move |_, resp| {
            resp_clone.set(resp);
            loop_clone.quit();
        });
        self.present();
        loop_.run();
        self.disconnect(handler_id);
        resp_cell.get()
    }
}
