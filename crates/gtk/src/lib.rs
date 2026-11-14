//! GTK compatibility shim providing GTK3 API ergonomics over GTK4.
#![allow(deprecated)]
#![allow(non_upper_case_globals)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::boxed::Box as StdBox;

pub use glib::object::Cast;
pub use glib::object::IsA;
pub use gdk4::prelude::*;
pub use gtk4::*;
pub use cairo;
pub use pango;

pub trait TreeViewColumnExt {
    fn pack_start<R: IsA<gtk4::CellRenderer>>(&self, cell: &R, expand: bool);
    fn add_attribute<R: IsA<gtk4::CellRenderer>>(&self, cell: &R, attribute: &str, column: i32);
}

impl TreeViewColumnExt for gtk4::TreeViewColumn {
    fn pack_start<R: IsA<gtk4::CellRenderer>>(&self, cell: &R, expand: bool) {
        gtk4::prelude::CellLayoutExt::pack_start(self, cell, expand);
    }
    fn add_attribute<R: IsA<gtk4::CellRenderer>>(&self, cell: &R, attribute: &str, column: i32) {
        gtk4::prelude::CellLayoutExt::add_attribute(self, cell, attribute, column);
    }
}

pub mod gdk {
    pub use gdk4::*;
    pub use super::keys;
    pub use super::CursorType;
    pub use super::EventMask;
    pub use super::EventKey;
    pub use super::EventButton;
    pub use super::EventType;
    pub use super::WindowState;
    pub use super::CompatGdkWindow;
    pub use super::SELECTION_CLIPBOARD;
    pub use super::SELECTION_PRIMARY;
    pub use super::Screen;

    pub fn set_program_class(_class: &str) {}
}

#[derive(Clone, Debug, Default)]
pub struct Screen;

impl Screen {
    pub fn default() -> Option<Self> {
        Some(Screen)
    }
}

pub mod keys {
    pub use gdk4::Key;
    #[allow(non_upper_case_globals)]
    pub mod constants {
        use gdk4::Key;

        // Letters lowercase
        pub const a: Key = Key::a;
        pub const b: Key = Key::b;
        pub const c: Key = Key::c;
        pub const d: Key = Key::d;
        pub const e: Key = Key::e;
        pub const f: Key = Key::f;
        pub const g: Key = Key::g;
        pub const h: Key = Key::h;
        pub const i: Key = Key::i;
        pub const j: Key = Key::j;
        pub const k: Key = Key::k;
        pub const l: Key = Key::l;
        pub const m: Key = Key::m;
        pub const n: Key = Key::n;
        pub const o: Key = Key::o;
        pub const p: Key = Key::p;
        pub const q: Key = Key::q;
        pub const r: Key = Key::r;
        pub const s: Key = Key::s;
        pub const t: Key = Key::t;
        pub const u: Key = Key::u;
        pub const v: Key = Key::v;
        pub const w: Key = Key::w;
        pub const x: Key = Key::x;
        pub const y: Key = Key::y;
        pub const z: Key = Key::z;

        // Letters uppercase
        pub const A: Key = Key::A;
        pub const B: Key = Key::B;
        pub const C: Key = Key::C;
        pub const D: Key = Key::D;
        pub const E: Key = Key::E;
        pub const F: Key = Key::F;
        pub const G: Key = Key::G;
        pub const H: Key = Key::H;
        pub const I: Key = Key::I;
        pub const J: Key = Key::J;
        pub const K: Key = Key::K;
        pub const L: Key = Key::L;
        pub const M: Key = Key::M;
        pub const N: Key = Key::N;
        pub const O: Key = Key::O;
        pub const P: Key = Key::P;
        pub const Q: Key = Key::Q;
        pub const R: Key = Key::R;
        pub const S: Key = Key::S;
        pub const T: Key = Key::T;
        pub const U: Key = Key::U;
        pub const V: Key = Key::V;
        pub const W: Key = Key::W;
        pub const X: Key = Key::X;
        pub const Y: Key = Key::Y;
        pub const Z: Key = Key::Z;

        // Numbers
        pub const _0: Key = Key::_0;
        pub const _1: Key = Key::_1;
        pub const _2: Key = Key::_2;
        pub const _3: Key = Key::_3;
        pub const _4: Key = Key::_4;
        pub const _5: Key = Key::_5;
        pub const _6: Key = Key::_6;
        pub const _7: Key = Key::_7;
        pub const _8: Key = Key::_8;
        pub const _9: Key = Key::_9;

        // Function keys
        pub const F1: Key = Key::F1;
        pub const F2: Key = Key::F2;
        pub const F3: Key = Key::F3;
        pub const F4: Key = Key::F4;
        pub const F5: Key = Key::F5;
        pub const F6: Key = Key::F6;
        pub const F7: Key = Key::F7;
        pub const F8: Key = Key::F8;
        pub const F9: Key = Key::F9;
        pub const F10: Key = Key::F10;
        pub const F11: Key = Key::F11;
        pub const F12: Key = Key::F12;

        // Navigation & Control
        pub const Left: Key = Key::Left;
        pub const Right: Key = Key::Right;
        pub const Up: Key = Key::Up;
        pub const Down: Key = Key::Down;
        pub const Home: Key = Key::Home;
        pub const End: Key = Key::End;
        pub const Page_Up: Key = Key::Page_Up;
        pub const Page_Down: Key = Key::Page_Down;
        pub const Tab: Key = Key::Tab;
        pub const ISO_Left_Tab: Key = Key::ISO_Left_Tab;
        pub const BackSpace: Key = Key::BackSpace;
        pub const Return: Key = Key::Return;
        pub const KP_Enter: Key = Key::KP_Enter;
        pub const Escape: Key = Key::Escape;
        pub const Delete: Key = Key::Delete;
        pub const Insert: Key = Key::Insert;

        // Punctuation & Symbols
        pub const minus: Key = Key::minus;
        pub const plus: Key = Key::plus;
        pub const equal: Key = Key::equal;
        pub const slash: Key = Key::slash;
        pub const period: Key = Key::period;
        pub const question: Key = Key::question;
        pub const greater: Key = Key::greater;
        pub const less: Key = Key::less;
        pub const comma: Key = Key::comma;
        pub const colon: Key = Key::colon;
        pub const semicolon: Key = Key::semicolon;
        pub const bracketleft: Key = Key::bracketleft;
        pub const bracketright: Key = Key::bracketright;
        pub const braceleft: Key = Key::braceleft;
        pub const braceright: Key = Key::braceright;
        pub const backslash: Key = Key::backslash;
        pub const bar: Key = Key::bar;
        pub const asciitilde: Key = Key::asciitilde;
        pub const grave: Key = Key::grave;
        pub const exclam: Key = Key::exclam;
        pub const at: Key = Key::at;
        pub const numbersign: Key = Key::numbersign;
        pub const dollar: Key = Key::dollar;
        pub const percent: Key = Key::percent;
        pub const asciicircum: Key = Key::asciicircum;
        pub const ampersand: Key = Key::ampersand;
        pub const asterisk: Key = Key::asterisk;
        pub const parenleft: Key = Key::parenleft;
        pub const parenright: Key = Key::parenright;
        pub const underscore: Key = Key::underscore;
        pub const space: Key = Key::space;
        pub const apostrophe: Key = Key::apostrophe;
        pub const quotedbl: Key = Key::quotedbl;
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ReliefStyle {
    Normal,
    Half,
    None,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShadowType {
    None,
    In,
    Out,
    EtchedIn,
    EtchedOut,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventType {
    ButtonPress,
    DoubleButtonPress,
    TripleButtonPress,
    ButtonRelease,
    KeyPress,
    KeyRelease,
    Other,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowType {
    Toplevel,
    Popup,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowPosition {
    None,
    Center,
    Mouse,
    CenterAlways,
    CenterOnParent,
}

pub fn window_new(_type: WindowType) -> gtk4::Window {
    gtk4::Window::new()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TargetFlags(pub u32);

impl TargetFlags {
    pub const SAME_APP: Self = Self(1 << 0);
    pub const SAME_WIDGET: Self = Self(1 << 1);
    pub const OTHER_APP: Self = Self(1 << 2);
    pub const OTHER_WIDGET: Self = Self(1 << 3);
}

impl std::ops::BitOr for TargetFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetEntry {
    pub target: String,
    pub flags: TargetFlags,
    pub info: u32,
}

impl TargetEntry {
    pub fn new(target: &str, flags: TargetFlags, info: u32) -> Self {
        Self {
            target: target.to_string(),
            flags,
            info,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DestDefaults(pub u32);

impl DestDefaults {
    pub const MOTION: Self = Self(1 << 0);
    pub const HIGHLIGHT: Self = Self(1 << 1);
    pub const DROP: Self = Self(1 << 2);
    pub const ALL: Self = Self(0x07);
}

impl std::ops::BitOr for DestDefaults {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EventMask(pub u32);

impl EventMask {
    pub const POINTER_MOTION_MASK: Self = Self(1 << 2);
    pub const BUTTON_PRESS_MASK: Self = Self(1 << 8);
    pub const BUTTON_RELEASE_MASK: Self = Self(1 << 9);
    pub const KEY_PRESS_MASK: Self = Self(1 << 10);
    pub const KEY_RELEASE_MASK: Self = Self(1 << 11);
    pub const ENTER_NOTIFY_MASK: Self = Self(1 << 12);
    pub const LEAVE_NOTIFY_MASK: Self = Self(1 << 13);
    pub const SCROLL_MASK: Self = Self(1 << 21);
    pub const ALL_EVENTS_MASK: Self = Self(0xFFFF_FFFF);
}

impl std::ops::BitOr for EventMask {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CursorType {
    XCursor,
    Arrow,
    Hand1,
    Hand2,
    Crosshair,
    Watch,
    Xterm,
    LeftPtr,
    RightPtr,
    CenterPtr,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WindowState(pub u32);

impl WindowState {
    pub const WITHDRAWN: Self = Self(1 << 0);
    pub const ICONIFIED: Self = Self(1 << 1);
    pub const MAXIMIZED: Self = Self(1 << 2);
    pub const STICKY: Self = Self(1 << 3);
    pub const FULLSCREEN: Self = Self(1 << 4);
    pub const ABOVE: Self = Self(1 << 5);
    pub const BELOW: Self = Self(1 << 6);
    pub const FOCUSED: Self = Self(1 << 7);
    pub const TILED: Self = Self(1 << 8);

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn is_fullscreen(&self) -> bool {
        self.contains(Self::FULLSCREEN)
    }

    pub fn is_iconified(&self) -> bool {
        self.contains(Self::ICONIFIED)
    }

    pub fn is_maximized(&self) -> bool {
        self.contains(Self::MAXIMIZED)
    }
}

impl std::ops::BitAnd for WindowState {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl std::ops::BitOr for WindowState {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Debug)]
pub struct CompatGdkWindow;

impl CompatGdkWindow {
    pub fn state(&self) -> WindowState {
        WindowState::empty()
    }
}

#[derive(Clone, Default, Debug)]
pub struct AccelGroup;

impl AccelGroup {
    pub fn new() -> Self {
        AccelGroup
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AccelFlags(pub u32);

impl AccelFlags {
    pub const VISIBLE: Self = Self(1);
}

impl std::ops::BitOr for AccelFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

pub type Container = gtk4::Widget;
pub type Bin = gtk4::Widget;

pub trait ContainerImpl: gtk4::subclass::prelude::WidgetImpl {}
pub trait BinImpl: ContainerImpl {}

pub mod glib_compat {
    pub use glib::*;
    pub use glib::object::IsA;
    use glib::translate::FromGlib;

    pub fn unix_signal_add_local<F: FnMut() -> glib::ControlFlow + 'static>(
        _signal: i32,
        _f: F,
    ) -> glib::SourceId {
        unsafe { glib::SourceId::from_glib(1) }
    }

    pub mod source {
        pub use glib::source::*;
        use glib::translate::FromGlib;

        pub fn unix_fd_add_local<F: FnMut(i32, glib::IOCondition) -> glib::ControlFlow + 'static>(
            _fd: i32,
            _condition: glib::IOCondition,
            _f: F,
        ) -> glib::SourceId {
            unsafe { glib::SourceId::from_glib(1) }
        }
    }
}

glib::wrapper! {
    pub struct EventBox(ObjectSubclass<imp::EventBox>)
        @extends gtk4::Box, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

mod imp {
    use super::*;
    use glib::subclass::prelude::*;
    use gtk4::subclass::prelude::*;

    #[derive(Default)]
    pub struct EventBox;

    #[glib::object_subclass]
    impl ObjectSubclass for EventBox {
        const NAME: &'static str = "OxtermCompatEventBox";
        type Type = super::EventBox;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for EventBox {}
    impl WidgetImpl for EventBox {}
    impl BoxImpl for EventBox {}
}

impl EventBox {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }
}

impl Default for EventBox {
    fn default() -> Self {
        Self::new()
    }
}

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
    pub fn popup_at_pointer(&self, _event: Option<&EventButton>) {}
    pub fn popup_at_widget<W: IsA<gtk4::Widget>>(
        &self,
        _widget: &W,
        _gravity1: gdk4::Gravity,
        _gravity2: gdk4::Gravity,
        _event: Option<&EventButton>,
    ) {}
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: String,
    pub action: Rc<RefCell<Option<StdBox<dyn Fn()>>>>,
    pub submenu: Rc<RefCell<Option<Menu>>>,
    pub sensitive: Rc<Cell<bool>>,
    pub tooltip: Rc<RefCell<Option<String>>>,
    pub is_separator: bool,
    pub check_state: Rc<Cell<Option<bool>>>,
    pub custom_widget: Rc<RefCell<Option<gtk4::Widget>>>,
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
            is_separator: false,
            check_state: Rc::new(Cell::new(None)),
            custom_widget: Rc::new(RefCell::new(None)),
        }
    }

    pub fn connect_activate<F: Fn(&MenuItem) + 'static>(&self, f: F) {
        let this = self.clone();
        *self.action.borrow_mut() = Some(StdBox::new(move || f(&this)));
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

    pub fn add<W: IsA<gtk4::Widget>>(&self, widget: &W) {
        *self.custom_widget.borrow_mut() = Some(widget.clone().upcast());
    }

    pub fn add_accelerator(
        &self,
        _signal: &str,
        _group: &AccelGroup,
        _key: gdk4::Key,
        _mods: gdk4::ModifierType,
        _flags: AccelFlags,
    ) {
    }

    pub fn destroy(&self) {}
}

#[derive(Clone)]
pub struct SeparatorMenuItem(pub MenuItem);

impl SeparatorMenuItem {
    pub fn new() -> Self {
        let mut item = MenuItem::new();
        item.is_separator = true;
        SeparatorMenuItem(item)
    }
}

impl std::ops::Deref for SeparatorMenuItem {
    type Target = MenuItem;
    fn deref(&self) -> &MenuItem {
        &self.0
    }
}

#[derive(Clone)]
pub struct CheckMenuItem(pub MenuItem);

impl CheckMenuItem {
    pub fn with_label(label: &str) -> Self {
        let mut item = MenuItem::with_label(label);
        item.check_state = Rc::new(Cell::new(Some(false)));
        CheckMenuItem(item)
    }

    pub fn is_active(&self) -> bool {
        self.0.check_state.get().unwrap_or(false)
    }

    pub fn set_active(&self, active: bool) {
        self.0.check_state.set(Some(active));
    }
}

impl std::ops::Deref for CheckMenuItem {
    type Target = MenuItem;
    fn deref(&self) -> &MenuItem {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct EventKey {
    pub keyval: gdk4::Key,
    pub state: gdk4::ModifierType,
}

impl EventKey {
    pub fn keyval(&self) -> gdk4::Key {
        self.keyval
    }
    pub fn state(&self) -> gdk4::ModifierType {
        self.state
    }
}

#[derive(Clone, Debug)]
pub struct EventButton {
    pub button: u32,
    pub n_press: i32,
    pub x: f64,
    pub y: f64,
    pub state: gdk4::ModifierType,
}

impl EventButton {
    pub fn button(&self) -> u32 {
        self.button
    }
    pub fn position(&self) -> (f64, f64) {
        (self.x, self.y)
    }
    pub fn event_type(&self) -> EventType {
        if self.n_press >= 2 {
            EventType::DoubleButtonPress
        } else {
            EventType::ButtonPress
        }
    }
    pub fn state(&self) -> gdk4::ModifierType {
        self.state
    }
}

pub struct Clipboard;

impl Clipboard {
    pub fn get(_atom: &str) -> Self {
        Clipboard
    }
    pub fn set_text(&self, text: &str) {
        if let Some(display) = gdk4::Display::default() {
            display.clipboard().set_text(text);
        }
    }
    pub fn request_text<F: FnOnce(Option<&str>) + 'static>(&self, callback: F) {
        if let Some(display) = gdk4::Display::default() {
            let clipboard = display.clipboard();
            clipboard.read_text_async(None::<&gtk4::gio::Cancellable>, move |result| {
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

pub mod prelude {
    pub use gtk4::prelude::*;
    pub use super::compat_traits::*;
    pub use super::TreeViewColumnExt;
}

pub mod compat_traits {
    use super::*;
    use gtk4::prelude::*;

    pub trait CompatBoxExt {
        fn pack_start<W: IsA<gtk4::Widget>>(&self, child: &W, expand: bool, fill: bool, padding: u32);
        fn pack_end<W: IsA<gtk4::Widget>>(&self, child: &W, expand: bool, fill: bool, padding: u32);
    }

    impl CompatBoxExt for gtk4::Box {
        fn pack_start<W: IsA<gtk4::Widget>>(&self, child: &W, expand: bool, _fill: bool, _padding: u32) {
            if expand {
                child.set_hexpand(true);
                child.set_vexpand(true);
            }
            self.append(child);
        }
        fn pack_end<W: IsA<gtk4::Widget>>(&self, child: &W, expand: bool, _fill: bool, _padding: u32) {
            if expand {
                child.set_hexpand(true);
                child.set_vexpand(true);
            }
            self.prepend(child);
        }
    }

    pub trait CompatPanedExt {
        fn pack1<W: IsA<gtk4::Widget>>(&self, child: &W, resize: bool, shrink: bool);
        fn pack2<W: IsA<gtk4::Widget>>(&self, child: &W, resize: bool, shrink: bool);
    }

    impl CompatPanedExt for gtk4::Paned {
        fn pack1<W: IsA<gtk4::Widget>>(&self, child: &W, resize: bool, shrink: bool) {
            self.set_start_child(Some(child));
            self.set_resize_start_child(resize);
            self.set_shrink_start_child(shrink);
        }
        fn pack2<W: IsA<gtk4::Widget>>(&self, child: &W, resize: bool, shrink: bool) {
            self.set_end_child(Some(child));
            self.set_resize_end_child(resize);
            self.set_shrink_end_child(shrink);
        }
    }

    pub trait CompatBinExt {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W);
    }

    impl CompatBinExt for gtk4::ScrolledWindow {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.set_child(Some(child));
        }
    }

    impl CompatBinExt for gtk4::Window {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.set_child(Some(child));
        }
    }

    impl CompatBinExt for gtk4::ApplicationWindow {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.set_child(Some(child));
        }
    }

    impl CompatBinExt for gtk4::Frame {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.set_child(Some(child));
        }
    }

    impl CompatBinExt for gtk4::Revealer {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.set_child(Some(child));
        }
    }

    impl CompatBinExt for gtk4::Button {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.set_child(Some(child));
        }
    }

    impl CompatBinExt for gtk4::Overlay {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.set_child(Some(child));
        }
    }

    impl CompatBinExt for gtk4::Box {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.append(child);
        }
    }

    impl CompatBinExt for EventBox {
        fn add<W: IsA<gtk4::Widget>>(&self, child: &W) {
            self.append(child);
        }
    }

    pub trait CompatWindowExt {
        fn set_position(&self, _pos: WindowPosition);
        fn add_accel_group(&self, _group: &AccelGroup) {}
    }

    impl<T: IsA<gtk4::Window>> CompatWindowExt for T {
        fn set_position(&self, _pos: WindowPosition) {}
        fn add_accel_group(&self, _group: &AccelGroup) {}
    }

    pub trait CompatButtonExt {
        fn set_relief(&self, relief: ReliefStyle);
    }

    impl CompatButtonExt for gtk4::Button {
        fn set_relief(&self, relief: ReliefStyle) {
            self.set_has_frame(relief != ReliefStyle::None);
        }
    }

    pub trait CompatScrolledWindowExt {
        fn set_shadow_type(&self, _shadow: ShadowType) {}
    }

    impl CompatScrolledWindowExt for gtk4::ScrolledWindow {}

    pub trait CompatFrameExt {
        fn set_shadow_type(&self, _shadow: ShadowType) {}
    }

    impl CompatFrameExt for gtk4::Frame {}

    pub trait CompatWidgetExt: IsA<gtk4::Widget> {
        fn show_all(&self) {
            self.set_visible(true);
        }
        fn set_no_show_all(&self, _no_show_all: bool) {}
        fn window(&self) -> Option<CompatGdkWindow> {
            Some(CompatGdkWindow)
        }
        fn remove<W: IsA<gtk4::Widget>>(&self, child: &W) {
            let widget = self.as_ref();
            if let Some(b) = widget.downcast_ref::<gtk4::Box>() {
                gtk4::prelude::BoxExt::remove(b, child);
            } else if let Some(w) = widget.downcast_ref::<gtk4::Window>() {
                w.set_child(None::<&gtk4::Widget>);
            }
        }
        fn style_context(&self) -> CompatStyleContext<'_, Self> {
            CompatStyleContext { widget: self }
        }
        fn destroy(&self) {
            let widget = self.as_ref();
            if let Some(w) = widget.downcast_ref::<gtk4::Window>() {
                w.close();
            } else if let Some(parent) = widget.parent() {
                if let Some(b) = parent.downcast_ref::<gtk4::Box>() {
                    gtk4::prelude::BoxExt::remove(b, widget);
                }
            }
        }
        fn add_events(&self, _mask: EventMask) {}
        fn drag_source_set(
            &self,
            _start_button_mask: gdk4::ModifierType,
            _targets: &[TargetEntry],
            _actions: gdk4::DragAction,
        ) {}
        fn drag_dest_set(
            &self,
            _flags: DestDefaults,
            _targets: &[TargetEntry],
            _actions: gdk4::DragAction,
        ) {}
        fn connect_button_press_event<F: Fn(&Self, &EventButton) -> glib::Propagation + 'static>(&self, f: F) {
            let gesture = gtk4::GestureClick::new();
            gesture.set_button(0);
            let widget = self.clone();
            gesture.connect_pressed(move |gesture, n_press, x, y| {
                let ev = EventButton {
                    button: gesture.current_button(),
                    n_press,
                    x,
                    y,
                    state: gesture.current_event_state(),
                };
                let _ = f(&widget, &ev);
            });
            self.add_controller(gesture);
        }
        fn connect_key_press_event<F: Fn(&Self, &EventKey) -> glib::Propagation + 'static>(&self, f: F) {
            let controller = gtk4::EventControllerKey::new();
            let widget = self.clone();
            controller.connect_key_pressed(move |_ctrl, keyval, _keycode, state| {
                let ev = EventKey { keyval, state };
                f(&widget, &ev)
            });
            self.add_controller(controller);
        }
    }

    impl<T: IsA<gtk4::Widget>> CompatWidgetExt for T {}

    pub struct CompatStyleContext<'a, W: ?Sized> {
        widget: &'a W,
    }

    impl<'a, W: IsA<gtk4::Widget>> CompatStyleContext<'a, W> {
        pub fn add_class(&self, class: &str) {
            self.widget.add_css_class(class);
        }
        pub fn remove_class(&self, class: &str) {
            self.widget.remove_css_class(class);
        }
        pub fn lookup_color(&self, _name: &str) -> Option<gdk4::RGBA> {
            Some(gdk4::RGBA::new(1.0, 1.0, 1.0, 1.0))
        }
        pub fn color(&self, _flags: gtk4::StateFlags) -> gdk4::RGBA {
            gdk4::RGBA::new(1.0, 1.0, 1.0, 1.0)
        }
    }

    pub trait CompatDialogExt {
        fn run(&self) -> gtk4::ResponseType;
    }

    impl<T: IsA<gtk4::Dialog> + IsA<gtk4::Window>> CompatDialogExt for T {
        fn run(&self) -> gtk4::ResponseType {
            let loop_ = glib::MainLoop::new(None, false);
            let resp_cell = Rc::new(Cell::new(gtk4::ResponseType::None));
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
}
