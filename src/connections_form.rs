//! Dialog that creates or edits a connection.
//!
//! The bottom of the dialog always shows the `ssh` command the entry produces:
//! Oxterm does not hide OpenSSH, and an option someone pasted from elsewhere
//! should be visible before it runs.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::headless::connections::{Auth, Connection, Kind, Tunnel, TunnelKind};

/// The result of a confirmed form.
pub struct FormResult {
    pub connection: Connection,
    /// A password typed for the keyring. `None` keeps what is stored.
    pub password: Option<String>,
}

struct TunnelRow {
    row: gtk::Box,
    kind: gtk::ComboBoxText,
    listen: gtk::Entry,
    target: gtk::Entry,
    autostart: gtk::CheckButton,
    /// Fields of a newer Oxterm, kept through an edit.
    original: Tunnel,
}

struct Fields {
    base: Connection,
    name: gtk::Entry,
    host: gtk::Entry,
    user: gtk::Entry,
    port: gtk::Entry,
    jump: gtk::Entry,
    group: gtk::Entry,
    tags: gtk::Entry,
    favorite: gtk::CheckButton,
    notes: gtk::TextView,
    identity: gtk::Entry,
    auth: gtk::ComboBoxText,
    password: gtk::Entry,
    remote_command: gtk::Entry,
    extra_args: gtk::Entry,
    confirm: gtk::CheckButton,
    tunnels: RefCell<Vec<TunnelRow>>,
}

impl Fields {
    fn tunnel_values(&self) -> Vec<Tunnel> {
        self.tunnels
            .borrow()
            .iter()
            .map(|row| Tunnel {
                kind: match row.kind.active_id().as_deref() {
                    Some("remote") => TunnelKind::Remote,
                    Some("dynamic") => TunnelKind::Dynamic,
                    _ => TunnelKind::Local,
                },
                listen: row.listen.text().trim().to_string(),
                target: row.target.text().trim().to_string(),
                autostart: row.autostart.is_active(),
                unknown: row.original.unknown.clone(),
            })
            .collect()
    }

    fn read(&self) -> Result<Connection, String> {
        let port_text = self.port.text();
        let port_text = port_text.trim();
        let port = if port_text.is_empty() {
            None
        } else {
            Some(
                port_text
                    .parse::<u16>()
                    .map_err(|_| format!("Invalid port: {}", port_text))?,
            )
        };
        let extra_args = shell_words::split(self.extra_args.text().trim())
            .map_err(|_| "Extra arguments have unbalanced quotes".to_string())?;
        let notes = {
            let buffer = self.notes.buffer().ok_or("Notes are unavailable")?;
            let (start, end) = buffer.bounds();
            buffer
                .text(&start, &end, false)
                .map(|text| text.to_string())
                .unwrap_or_default()
        };
        let mut connection = self.base.clone();
        connection.name = self.name.text().trim().to_string();
        connection.group = self.group.text().trim().to_string();
        connection.tags = self
            .tags
            .text()
            .split([',', ' '])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect();
        connection.favorite = self.favorite.is_active();
        connection.notes = notes;
        connection.remote_command = self.remote_command.text().trim().to_string();
        connection.extra_args = extra_args;
        connection.confirm_connect = self.confirm.is_active();
        connection.tunnels = self.tunnel_values();
        if connection.kind == Kind::Manual {
            connection.host = self.host.text().trim().to_string();
            connection.user = self.user.text().trim().to_string();
            connection.port = port;
            connection.proxy_jump = self.jump.text().trim().to_string();
            connection.identity_file = self.identity.text().trim().to_string();
        }
        connection.auth = match self.auth.active_id().as_deref() {
            Some("secret_service") => Auth::SecretService,
            _ => Auth::Default,
        };
        Ok(connection)
    }
}

fn grid() -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(12);
    grid.set_margin_bottom(12);
    grid
}

fn add_row(grid: &gtk::Grid, row: i32, label: &str, widget: &impl IsA<gtk::Widget>) {
    let caption = gtk::Label::new(Some(label));
    caption.set_xalign(1.0);
    grid.attach(&caption, 0, row, 1, 1);
    widget.set_hexpand(true);
    grid.attach(widget, 1, row, 1, 1);
}

fn entry(value: &str, placeholder: &str) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.set_text(value);
    entry.set_placeholder_text(Some(placeholder));
    entry.set_activates_default(true);
    entry
}

fn add_tunnel_row(fields: &Rc<Fields>, list: &gtk::Box, tunnel: Tunnel, refresh: &Rc<dyn Fn()>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let kind = gtk::ComboBoxText::new();
    kind.append(Some("local"), "Local (-L)");
    kind.append(Some("remote"), "Remote (-R)");
    kind.append(Some("dynamic"), "SOCKS (-D)");
    kind.set_active_id(Some(match tunnel.kind {
        TunnelKind::Local => "local",
        TunnelKind::Remote => "remote",
        TunnelKind::Dynamic => "dynamic",
    }));
    let listen = entry(&tunnel.listen, "[bind:]port");
    listen.set_width_chars(16);
    let target = entry(&tunnel.target, "host:port");
    target.set_width_chars(20);
    target.set_hexpand(true);
    target.set_sensitive(tunnel.kind != TunnelKind::Dynamic);
    let autostart = gtk::CheckButton::with_label("Autostart");
    autostart.set_active(tunnel.autostart);
    autostart.set_tooltip_text(Some("Start this tunnel when the connection opens"));
    let remove = gtk::Button::with_label("Remove");
    row.pack_start(&kind, false, false, 0);
    row.pack_start(&listen, false, false, 0);
    row.pack_start(&target, true, true, 0);
    row.pack_start(&autostart, false, false, 0);
    row.pack_start(&remove, false, false, 0);
    list.pack_start(&row, false, false, 0);
    row.show_all();

    let target_for_kind = target.clone();
    let refresh_for_kind = refresh.clone();
    kind.connect_changed(move |combo| {
        target_for_kind.set_sensitive(combo.active_id().as_deref() != Some("dynamic"));
        refresh_for_kind();
    });
    for field in [&listen, &target] {
        let refresh = refresh.clone();
        field.connect_changed(move |_| refresh());
    }
    let weak_fields = Rc::downgrade(fields);
    let row_for_remove = row.clone();
    let refresh_for_remove = refresh.clone();
    remove.connect_clicked(move |_| {
        if let Some(fields) = weak_fields.upgrade() {
            fields
                .tunnels
                .borrow_mut()
                .retain(|item| item.row != row_for_remove);
        }
        // SAFETY of the GTK contract: the row is removed from its parent and
        // holds no references the form still uses.
        unsafe {
            row_for_remove.destroy();
        }
        refresh_for_remove();
    });
    fields.tunnels.borrow_mut().push(TunnelRow {
        row,
        kind,
        listen,
        target,
        autostart,
        original: tunnel,
    });
}

/// Show the form until it is confirmed with valid data or cancelled. `check`
/// lets the caller reject what only it can see, such as a duplicate name.
pub fn edit(
    parent: &gtk::Window,
    title: &str,
    initial: Connection,
    keyring_available: bool,
    check: &dyn Fn(&Connection) -> Result<(), String>,
) -> Option<FormResult> {
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Save", gtk::ResponseType::Accept),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Accept);
    dialog.set_default_size(680, 480);
    let is_alias = initial.kind == Kind::Alias;

    let notes = gtk::TextView::new();
    notes.set_wrap_mode(gtk::WrapMode::WordChar);
    if let Some(buffer) = notes.buffer() {
        buffer.set_text(&initial.notes);
    }
    let auth = gtk::ComboBoxText::new();
    auth.append(Some("default"), "Keys, agent, or the ssh prompt");
    auth.append(Some("secret_service"), "Password in the system keyring");
    auth.set_active_id(Some(match initial.auth {
        Auth::Default => "default",
        Auth::SecretService => "secret_service",
    }));
    auth.set_sensitive(keyring_available || initial.auth == Auth::SecretService);
    let password = entry("", "Leave empty to keep the stored password");
    password.set_visibility(false);
    password.set_input_purpose(gtk::InputPurpose::Password);
    password.set_sensitive(initial.auth == Auth::SecretService);

    let favorite = gtk::CheckButton::with_label("Show in Favorites");
    favorite.set_active(initial.favorite);
    let confirm = gtk::CheckButton::with_label("Ask before connecting");
    confirm.set_active(initial.confirm_connect);
    let fields = Rc::new(Fields {
        base: initial.clone(),
        name: entry(&initial.name, "db-prod-1"),
        host: entry(&initial.host, "server.example or user@server"),
        user: entry(&initial.user, "Default from ~/.ssh/config"),
        port: entry(
            &initial
                .port
                .map(|port| port.to_string())
                .unwrap_or_default(),
            "22",
        ),
        jump: entry(&initial.proxy_jump, "bastion"),
        group: entry(&initial.group, "Customers/ACME"),
        tags: entry(&initial.tags.join(", "), "db, prod"),
        favorite,
        notes,
        identity: entry(&initial.identity_file, "~/.ssh/id_ed25519"),
        auth,
        password,
        remote_command: entry(&initial.remote_command, "tmux new -A -s main"),
        extra_args: entry(
            &shell_words::join(&initial.extra_args),
            "-C -o Compression=yes",
        ),
        confirm,
        tunnels: RefCell::new(Vec::new()),
    });

    // ── Connection ──
    let connection_page = grid();
    let mut row = 0;
    add_row(&connection_page, row, "Name", &fields.name);
    row += 1;
    if is_alias {
        // The name of an alias *is* the Host entry; renaming it here would
        // silently disconnect it from ~/.ssh/config.
        fields.name.set_sensitive(false);
        let note = gtk::Label::new(Some(
            "This host comes from ~/.ssh/config, which Oxterm never changes. \
             Edit its address, user and keys there; here you only add organization.",
        ));
        note.set_xalign(0.0);
        note.set_line_wrap(true);
        note.style_context().add_class("dim-label");
        connection_page.attach(&note, 0, row, 2, 1);
        row += 1;
    } else {
        add_row(&connection_page, row, "Host", &fields.host);
        row += 1;
        add_row(&connection_page, row, "User", &fields.user);
        row += 1;
        add_row(&connection_page, row, "Port", &fields.port);
        row += 1;
        add_row(&connection_page, row, "ProxyJump", &fields.jump);
        row += 1;
    }
    add_row(&connection_page, row, "Group", &fields.group);
    row += 1;
    add_row(&connection_page, row, "Tags", &fields.tags);
    row += 1;
    connection_page.attach(&fields.favorite, 1, row, 1, 1);
    row += 1;
    let notes_scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    notes_scroll.set_shadow_type(gtk::ShadowType::In);
    notes_scroll.set_vexpand(true);
    notes_scroll.set_min_content_height(64);
    notes_scroll.add(&fields.notes);
    add_row(&connection_page, row, "Notes", &notes_scroll);

    // ── Authentication ──
    let auth_page = grid();
    let mut row = 0;
    if !is_alias {
        add_row(&auth_page, row, "Identity file", &fields.identity);
        row += 1;
    }
    add_row(&auth_page, row, "Method", &fields.auth);
    row += 1;
    add_row(&auth_page, row, "Password", &fields.password);
    row += 1;
    let auth_text = if keyring_available {
        "Keys and an agent are the better choice when the server allows them. A stored \
         password lives in the system keyring, never in Oxterm's files. It is answered only \
         when ssh asks for this exact user and host: a jump host never receives it, and \
         host-key questions and passphrases stay in the terminal."
    } else {
        "secret-tool was not found, so passwords cannot be stored. ssh will ask in the \
         terminal."
    };
    let auth_note = gtk::Label::new(Some(auth_text));
    auth_note.set_xalign(0.0);
    auth_note.set_line_wrap(true);
    auth_note.style_context().add_class("dim-label");
    auth_page.attach(&auth_note, 0, row, 2, 1);

    // ── Tunnels ──
    let tunnel_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    tunnel_page.set_margin_start(12);
    tunnel_page.set_margin_end(12);
    tunnel_page.set_margin_top(12);
    tunnel_page.set_margin_bottom(12);
    let tunnel_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    tunnel_page.pack_start(&tunnel_list, false, false, 0);
    let add_tunnel = gtk::Button::with_label("Add Tunnel");
    add_tunnel.set_halign(gtk::Align::Start);
    tunnel_page.pack_start(&add_tunnel, false, false, 0);
    let tunnel_note = gtk::Label::new(Some(
        "Each tunnel runs its own background ssh. It cannot prompt, so it needs a key or an \
         agent, or a stored password for this exact host.",
    ));
    tunnel_note.set_xalign(0.0);
    tunnel_note.set_line_wrap(true);
    tunnel_note.style_context().add_class("dim-label");
    tunnel_page.pack_start(&tunnel_note, false, false, 0);

    // ── Advanced ──
    let advanced_page = grid();
    add_row(&advanced_page, 0, "Remote command", &fields.remote_command);
    add_row(&advanced_page, 1, "Extra ssh arguments", &fields.extra_args);
    advanced_page.attach(&fields.confirm, 1, 2, 1, 1);

    let notebook = gtk::Notebook::new();
    notebook.append_page(&connection_page, Some(&gtk::Label::new(Some("Connection"))));
    notebook.append_page(&auth_page, Some(&gtk::Label::new(Some("Authentication"))));
    notebook.append_page(&tunnel_page, Some(&gtk::Label::new(Some("Tunnels"))));
    notebook.append_page(&advanced_page, Some(&gtk::Label::new(Some("Advanced"))));

    let preview = gtk::Label::new(None);
    preview.set_xalign(0.0);
    preview.set_selectable(true);
    preview.set_line_wrap(true);
    preview.set_line_wrap_mode(pango::WrapMode::WordChar);
    preview.style_context().add_class("monospace");
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.set_line_wrap(true);
    let copy = gtk::Button::with_label("Copy");
    copy.set_tooltip_text(Some("Copy the command"));
    let preview_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    preview_row.pack_start(&preview, true, true, 0);
    preview_row.pack_end(&copy, false, false, 0);
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 4);
    footer.set_margin_start(12);
    footer.set_margin_end(12);
    footer.set_margin_top(8);
    footer.set_margin_bottom(8);
    footer.pack_start(&preview_row, false, false, 0);
    footer.pack_start(&error, false, false, 0);

    let content = dialog.content_area();
    content.pack_start(&notebook, true, true, 0);
    content.pack_start(&footer, false, false, 0);

    // A problem shows up while typing, not only after Save. An option that runs
    // a local program is flagged even when valid: the entry may be imported.
    let refresh: Rc<dyn Fn()> = {
        let fields = Rc::downgrade(&fields);
        let preview = preview.clone();
        let error = error.clone();
        Rc::new(move || {
            let Some(fields) = fields.upgrade() else {
                return;
            };
            match fields.read().and_then(|connection| {
                connection.validate()?;
                Ok(connection)
            }) {
                Ok(connection) => {
                    preview.set_text(&connection.command_preview());
                    let risky = connection.local_execution_options();
                    if risky.is_empty() {
                        error.set_text("");
                    } else {
                        error.set_text(&format!(
                            "These options run a local program: {}",
                            risky.join(" ")
                        ));
                    }
                }
                Err(message) => error.set_text(&message),
            }
        })
    };
    for field in [
        &fields.name,
        &fields.host,
        &fields.user,
        &fields.port,
        &fields.jump,
        &fields.identity,
        &fields.remote_command,
        &fields.extra_args,
    ] {
        let refresh = refresh.clone();
        field.connect_changed(move |_| refresh());
    }
    {
        let password = fields.password.clone();
        fields.auth.connect_changed(move |combo| {
            password.set_sensitive(combo.active_id().as_deref() == Some("secret_service"));
        });
    }
    for tunnel in &initial.tunnels {
        add_tunnel_row(&fields, &tunnel_list, tunnel.clone(), &refresh);
    }
    {
        let fields = fields.clone();
        let list = tunnel_list.clone();
        let refresh = refresh.clone();
        add_tunnel.connect_clicked(move |_| {
            add_tunnel_row(&fields, &list, Tunnel::default(), &refresh);
            refresh();
        });
    }
    {
        let preview = preview.clone();
        copy.connect_clicked(move |_| {
            gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&preview.text());
        });
    }
    refresh();
    dialog.show_all();
    if fields.name.is_sensitive() {
        fields.name.grab_focus();
    } else {
        fields.group.grab_focus();
    }

    let result = loop {
        if dialog.run() != gtk::ResponseType::Accept {
            break None;
        }
        let outcome = fields.read().and_then(|connection| {
            connection.validate()?;
            check(&connection)?;
            Ok(connection)
        });
        match outcome {
            Ok(connection) => {
                let typed = fields.password.text().to_string();
                let password =
                    (connection.auth == Auth::SecretService && !typed.is_empty()).then_some(typed);
                break Some(FormResult {
                    connection,
                    password,
                });
            }
            Err(message) => {
                error.set_text(&message);
            }
        }
    };
    // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
    unsafe {
        dialog.destroy();
    }
    result
}
