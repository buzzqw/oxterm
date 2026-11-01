//! Dialog that creates or edits a connection.
//!
//! The bottom of the dialog always shows the `ssh` command the entry produces:
//! Oxterm does not hide OpenSSH, and an option someone pasted from elsewhere
//! should be visible before it runs.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::headless::connections::{
    Auth, Connection, ExpectRule, HostKeyCheck, Kind, Macro, Tunnel, TunnelKind,
};
use crate::headless::totp;

/// The result of a confirmed form.
pub struct FormResult {
    pub connection: Connection,
    /// A password typed for the keyring. `None` keeps what is stored.
    pub password: Option<String>,
    /// A one-time code secret for the keyring. `None` keeps what is stored.
    pub totp_secret: Option<String>,
}

struct MacroRow {
    row: gtk::Box,
    name: gtk::Entry,
    command: gtk::Entry,
}

struct ExpectRow {
    row: gtk::Box,
    pattern: gtk::Entry,
    command: gtk::Entry,
    max_trigger: gtk::SpinButton,
    delay: gtk::SpinButton,
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
    color_scheme: gtk::ComboBoxText,
    tunnels: RefCell<Vec<TunnelRow>>,
    macros: RefCell<Vec<MacroRow>>,
    expect: RefCell<Vec<ExpectRow>>,
    x11: gtk::CheckButton,
    compression: gtk::CheckButton,
    agent_forward: gtk::CheckButton,
    no_sharing: gtk::CheckButton,
    legacy_kex: gtk::CheckButton,
    legacy_cipher: gtk::CheckButton,
    legacy_hostkey: gtk::CheckButton,
    legacy_mac: gtk::CheckButton,
    legacy_pubkey: gtk::CheckButton,
    keepalive: gtk::Entry,
    connect_timeout: gtk::Entry,
    host_key: gtk::ComboBoxText,
    terminal_type: gtk::Entry,
    keep_shell: gtk::CheckButton,
    pre_command: gtk::Entry,
    pre_timeout: gtk::Entry,
    wol_mac: gtk::Entry,
    wol_wait: gtk::Entry,
    auto_reconnect: gtk::CheckButton,
    reconnect_delay: gtk::Entry,
    log_output: gtk::CheckButton,
    log_dir: gtk::Entry,
    totp: gtk::CheckButton,
    totp_secret: gtk::Entry,
    font: gtk::Entry,
    scrollback: gtk::Entry,
    encoding: gtk::Entry,
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

    fn macro_values(&self) -> Vec<Macro> {
        self.macros
            .borrow()
            .iter()
            .map(|row| Macro {
                name: row.name.text().trim().to_string(),
                command: row.command.text().trim().to_string(),
            })
            .collect()
    }

    fn expect_values(&self) -> Vec<ExpectRule> {
        self.expect
            .borrow()
            .iter()
            .map(|row| ExpectRule {
                pattern: row.pattern.text().to_string(),
                command: row.command.text().to_string(),
                max_trigger: row.max_trigger.value_as_int().max(1) as u32,
                delay_ms: (row.delay.value() * 1000.0) as u64,
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
        connection.color_scheme = self
            .color_scheme
            .active_id()
            .map(|id| id.to_string())
            .unwrap_or_default();
        connection.tunnels = self.tunnel_values();
        connection.macros = self.macro_values();
        connection.expect = self.expect_values();
        connection.x11 = self.x11.is_active();
        connection.compression = self.compression.is_active();
        connection.agent_forward = self.agent_forward.is_active();
        connection.no_sharing = self.no_sharing.is_active();
        connection.legacy_kex = self.legacy_kex.is_active();
        connection.legacy_cipher = self.legacy_cipher.is_active();
        connection.legacy_hostkey = self.legacy_hostkey.is_active();
        connection.legacy_mac = self.legacy_mac.is_active();
        connection.legacy_pubkey = self.legacy_pubkey.is_active();
        connection.keepalive_interval = optional_number(&self.keepalive, "Keepalive")?;
        connection.connect_timeout = optional_number(&self.connect_timeout, "Connect timeout")?;
        connection.host_key_check = match self.host_key.active_id().as_deref() {
            Some("ask") => HostKeyCheck::Ask,
            Some("accept_new") => HostKeyCheck::AcceptNew,
            Some("strict") => HostKeyCheck::Strict,
            _ => HostKeyCheck::Default,
        };
        connection.terminal_type = self.terminal_type.text().trim().to_string();
        connection.keep_shell = self.keep_shell.is_active();
        connection.pre_command = self.pre_command.text().trim().to_string();
        connection.pre_command_timeout = optional_number(&self.pre_timeout, "Pre-connect timeout")?;
        connection.wol_mac = self.wol_mac.text().trim().to_string();
        connection.wol_wait = optional_number(&self.wol_wait, "Wake-on-LAN wait")?.unwrap_or(0);
        connection.auto_reconnect = self.auto_reconnect.is_active();
        connection.reconnect_delay = optional_number(&self.reconnect_delay, "Reconnect delay")?;
        connection.log_output = self.log_output.is_active();
        connection.log_dir = self.log_dir.text().trim().to_string();
        connection.totp = self.totp.is_active();
        connection.font = self.font.text().trim().to_string();
        connection.scrollback =
            optional_number::<u32>(&self.scrollback, "Scrollback")?.map(i64::from);
        connection.encoding = self.encoding.text().trim().to_string();
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

/// An empty field means "not set"; anything else must be a number.
fn optional_number<T: std::str::FromStr>(
    field: &gtk::Entry,
    label: &str,
) -> Result<Option<T>, String> {
    let text = field.text();
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| format!("{} must be a number: {}", label, text))
}

fn option_check(label: &str, active: bool, tooltip: &str) -> gtk::CheckButton {
    let button = gtk::CheckButton::with_label(label);
    button.set_active(active);
    button.set_tooltip_text(Some(tooltip));
    button
}

fn number_text(value: Option<impl ToString>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn note(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_line_wrap(true);
    label.style_context().add_class("dim-label");
    label
}

/// A page that scrolls: small screens must still reach its last row.
fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.add(child);
    scroll
}

fn list_page(add_label: &str, explanation: &str) -> (gtk::Box, gtk::Box, gtk::Button) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_margin_start(12);
    page.set_margin_end(12);
    page.set_margin_top(12);
    page.set_margin_bottom(12);
    page.pack_start(&note(explanation), false, false, 0);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.pack_start(&list, false, false, 0);
    let add = gtk::Button::with_label(add_label);
    add.set_halign(gtk::Align::Start);
    page.pack_start(&add, false, false, 0);
    (page, list, add)
}

fn add_macro_row(fields: &Rc<Fields>, list: &gtk::Box, item: Macro) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let name = entry(&item.name, "Service status");
    name.set_width_chars(18);
    let command = entry(&item.command, "systemctl status nginx");
    command.set_hexpand(true);
    let remove = gtk::Button::with_label("Remove");
    row.pack_start(&name, false, false, 0);
    row.pack_start(&command, true, true, 0);
    row.pack_start(&remove, false, false, 0);
    list.pack_start(&row, false, false, 0);
    row.show_all();
    let weak = Rc::downgrade(fields);
    let row_for_remove = row.clone();
    remove.connect_clicked(move |_| {
        if let Some(fields) = weak.upgrade() {
            fields
                .macros
                .borrow_mut()
                .retain(|item| item.row != row_for_remove);
        }
        // SAFETY of the GTK contract: the form keeps no other use of this row.
        unsafe {
            row_for_remove.destroy();
        }
    });
    fields
        .macros
        .borrow_mut()
        .push(MacroRow { row, name, command });
}

fn add_expect_row(fields: &Rc<Fields>, list: &gtk::Box, rule: ExpectRule) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let pattern = entry(&rule.pattern, "Regular expression");
    pattern.set_width_chars(20);
    let command = entry(&rule.command, "Command to type");
    command.set_hexpand(true);
    let max_trigger = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
    max_trigger.set_value(f64::from(rule.max_trigger));
    max_trigger.set_tooltip_text(Some("How many times this rule may fire"));
    let delay = gtk::SpinButton::with_range(0.0, 60.0, 0.5);
    delay.set_digits(1);
    delay.set_value(rule.delay_ms as f64 / 1000.0);
    delay.set_tooltip_text(Some("Seconds to wait before typing"));
    let remove = gtk::Button::with_label("Remove");
    row.pack_start(&pattern, false, false, 0);
    row.pack_start(&command, true, true, 0);
    row.pack_start(&max_trigger, false, false, 0);
    row.pack_start(&delay, false, false, 0);
    row.pack_start(&remove, false, false, 0);
    list.pack_start(&row, false, false, 0);
    row.show_all();
    let weak = Rc::downgrade(fields);
    let row_for_remove = row.clone();
    remove.connect_clicked(move |_| {
        if let Some(fields) = weak.upgrade() {
            fields
                .expect
                .borrow_mut()
                .retain(|item| item.row != row_for_remove);
        }
        // SAFETY of the GTK contract: the form keeps no other use of this row.
        unsafe {
            row_for_remove.destroy();
        }
    });
    fields.expect.borrow_mut().push(ExpectRow {
        row,
        pattern,
        command,
        max_trigger,
        delay,
    });
}

/// Private keys in `~/.ssh`: every `name` that has a `name.pub` next to it.
fn known_keys() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut keys: Vec<String> = std::fs::read_dir(home.join(".ssh"))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let stem = name.strip_suffix(".pub")?.to_string();
                    entry
                        .path()
                        .with_file_name(&stem)
                        .is_file()
                        .then(|| format!("~/.ssh/{}", stem))
                })
                .collect()
        })
        .unwrap_or_default();
    keys.sort();
    keys
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
    let caption = gtk::Label::with_mnemonic(label);
    caption.set_mnemonic_widget(Some(widget));
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
    dialog.set_default_size(780, 560);
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
    let color_scheme = gtk::ComboBoxText::new();
    color_scheme.append(Some(""), "Same as the other tabs");
    for name in crate::settings::color_schemes().keys() {
        color_scheme.append(Some(name), name);
    }
    if !initial.color_scheme.is_empty()
        && !crate::settings::color_schemes().contains_key(&initial.color_scheme)
    {
        // A theme file that is gone: keep the choice rather than erase it.
        color_scheme.append(Some(&initial.color_scheme), &initial.color_scheme);
    }
    color_scheme.set_active_id(Some(&initial.color_scheme));
    color_scheme.set_tooltip_text(Some(
        "Recognize this server at a glance: only its tabs use this scheme",
    ));
    // The combo box owns its entry, which the form treats like any other field.
    let keys = gtk::ComboBoxText::with_entry();
    for key in known_keys() {
        keys.append_text(&key);
    }
    let identity = keys
        .child()
        .and_then(|child| child.downcast::<gtk::Entry>().ok())
        .unwrap_or_default();
    identity.set_text(&initial.identity_file);
    identity.set_placeholder_text(Some("~/.ssh/id_ed25519"));
    identity.set_activates_default(true);
    let host_key = gtk::ComboBoxText::new();
    host_key.append(Some("default"), "As in my ssh config");
    host_key.append(Some("ask"), "Ask about new hosts");
    host_key.append(Some("accept_new"), "Accept new hosts, refuse changed keys");
    host_key.append(Some("strict"), "Refuse unknown hosts");
    host_key.set_active_id(Some(match initial.host_key_check {
        HostKeyCheck::Default => "default",
        HostKeyCheck::Ask => "ask",
        HostKeyCheck::AcceptNew => "accept_new",
        HostKeyCheck::Strict => "strict",
    }));
    let totp_secret = entry("", "Leave empty to keep the stored secret");
    totp_secret.set_visibility(false);
    totp_secret.set_sensitive(initial.totp);
    totp_secret.set_tooltip_text(Some(
        "The Base32 secret, or the otpauth:// link, from your authenticator setup",
    ));
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
        identity,
        auth,
        password,
        remote_command: entry(&initial.remote_command, "tmux new -A -s main"),
        extra_args: entry(
            &shell_words::join(&initial.extra_args),
            "-C -o Compression=yes",
        ),
        confirm,
        color_scheme,
        tunnels: RefCell::new(Vec::new()),
        macros: RefCell::new(Vec::new()),
        expect: RefCell::new(Vec::new()),
        x11: option_check(
            "X11 forwarding (-X)",
            initial.x11,
            "Show remote graphical programs on this display",
        ),
        compression: option_check(
            "Compression (-C)",
            initial.compression,
            "Useful on slow links",
        ),
        agent_forward: option_check(
            "Agent forwarding (-A)",
            initial.agent_forward,
            "The server can use your agent while you are connected. Prefer ProxyJump for hopping.",
        ),
        no_sharing: option_check(
            "Do not share this connection",
            initial.no_sharing,
            "By default tabs to the same host share one login. Turn that off for this entry.",
        ),
        legacy_kex: option_check(
            "Key exchange",
            initial.legacy_kex,
            "diffie-hellman-group1-sha1, group14-sha1",
        ),
        legacy_cipher: option_check(
            "Ciphers",
            initial.legacy_cipher,
            "aes128-cbc, aes256-cbc, 3des-cbc",
        ),
        legacy_hostkey: option_check("Host keys", initial.legacy_hostkey, "ssh-rsa, ssh-dss"),
        legacy_mac: option_check("MACs", initial.legacy_mac, "hmac-sha1, hmac-md5"),
        legacy_pubkey: option_check("Public keys", initial.legacy_pubkey, "ssh-rsa"),
        keepalive: entry(
            &number_text(initial.keepalive_interval),
            "Default: 30, from Oxterm or your ssh config",
        ),
        connect_timeout: entry(
            &number_text(initial.connect_timeout),
            "Default: from your ssh config",
        ),
        host_key,
        terminal_type: entry(&initial.terminal_type, "xterm-256color"),
        keep_shell: option_check(
            "Stay in a shell after the command",
            initial.keep_shell,
            "Run the command, then continue with a login shell",
        ),
        pre_command: entry(&initial.pre_command, "wg-quick up vpn0"),
        pre_timeout: entry(
            &number_text(initial.pre_command_timeout),
            "15 (0 waits as long as it takes)",
        ),
        wol_mac: entry(&initial.wol_mac, "aa:bb:cc:dd:ee:ff"),
        wol_wait: entry(
            &(if initial.wol_wait > 0 {
                initial.wol_wait.to_string()
            } else {
                String::new()
            }),
            "Seconds to wait for the host",
        ),
        auto_reconnect: option_check(
            "Reconnect when the connection drops",
            initial.auto_reconnect,
            "Only when ssh loses the connection: exit and a failed command do not reconnect",
        ),
        reconnect_delay: entry(&number_text(initial.reconnect_delay), "5"),
        log_output: option_check(
            "Record the output to a file",
            initial.log_output,
            "A new file for each connection",
        ),
        log_dir: entry(&initial.log_dir, "~/ssh-logs"),
        totp: option_check(
            "Answer one-time codes",
            initial.totp,
            "Only a code prompt that names this user and host. The secret stays in the keyring.",
        ),
        totp_secret,
        font: entry(
            &initial.font,
            "Monospace 12 (default: same as the other tabs)",
        ),
        scrollback: entry(
            &number_text(initial.scrollback),
            "Default: same as the other tabs",
        ),
        encoding: entry(
            &initial.encoding,
            "ISO-8859-1 (default: same as the other tabs)",
        ),
    });

    // ── Connection ──
    let connection_page = grid();
    let mut row = 0;
    add_row(&connection_page, row, "_Name", &fields.name);
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
        add_row(&connection_page, row, "_Host", &fields.host);
        row += 1;
        add_row(&connection_page, row, "_User", &fields.user);
        row += 1;
        add_row(&connection_page, row, "Po_rt", &fields.port);
        row += 1;
        add_row(&connection_page, row, "Proxy_Jump", &fields.jump);
        row += 1;
    }
    add_row(&connection_page, row, "_Group", &fields.group);
    row += 1;
    add_row(&connection_page, row, "_Tags", &fields.tags);
    row += 1;
    connection_page.attach(&fields.favorite, 1, row, 1, 1);
    row += 1;
    let notes_scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    notes_scroll.set_shadow_type(gtk::ShadowType::In);
    notes_scroll.set_vexpand(true);
    notes_scroll.set_min_content_height(64);
    notes_scroll.add(&fields.notes);
    add_row(&connection_page, row, "N_otes", &notes_scroll);

    // ── Authentication ──
    let auth_page = grid();
    let mut row = 0;
    if !is_alias {
        keys.set_tooltip_text(Some("Keys found in ~/.ssh, or type another path"));
        add_row(&auth_page, row, "_Identity file", &keys);
        row += 1;
    }
    add_row(&auth_page, row, "_Method", &fields.auth);
    row += 1;
    add_row(&auth_page, row, "_Password", &fields.password);
    row += 1;
    auth_page.attach(&fields.totp, 1, row, 1, 1);
    row += 1;
    add_row(&auth_page, row, "One-time _secret", &fields.totp_secret);
    row += 1;
    {
        let secret = fields.totp_secret.clone();
        fields
            .totp
            .connect_toggled(move |button| secret.set_sensitive(button.is_active()));
    }
    fields.totp.set_sensitive(keyring_available || initial.totp);
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

    // ── SSH ──
    let ssh_page = grid();
    let mut row = 0;
    for button in [
        &fields.x11,
        &fields.compression,
        &fields.agent_forward,
        &fields.no_sharing,
    ] {
        ssh_page.attach(button, 1, row, 1, 1);
        row += 1;
    }
    add_row(&ssh_page, row, "_Keepalive (s)", &fields.keepalive);
    row += 1;
    add_row(
        &ssh_page,
        row,
        "Connect _timeout (s)",
        &fields.connect_timeout,
    );
    row += 1;
    add_row(&ssh_page, row, "_Unknown hosts", &fields.host_key);
    row += 1;
    add_row(&ssh_page, row, "Termin_al type", &fields.terminal_type);
    row += 1;
    add_row(&ssh_page, row, "_Extra ssh arguments", &fields.extra_args);
    row += 1;
    let legacy = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for button in [
        &fields.legacy_kex,
        &fields.legacy_cipher,
        &fields.legacy_hostkey,
        &fields.legacy_mac,
        &fields.legacy_pubkey,
    ] {
        legacy.pack_start(button, false, false, 0);
    }
    let legacy_label = gtk::Label::new(Some("Old algorithms"));
    legacy_label.set_xalign(1.0);
    legacy_label.set_tooltip_text(Some("For routers, NAS boxes and other aged servers"));
    ssh_page.attach(&legacy_label, 0, row, 1, 1);
    ssh_page.attach(&legacy, 1, row, 1, 1);

    // ── Session ──
    let session_page = grid();
    let mut row = 0;
    add_row(
        &session_page,
        row,
        "Remote _command",
        &fields.remote_command,
    );
    row += 1;
    session_page.attach(&fields.keep_shell, 1, row, 1, 1);
    row += 1;
    add_row(
        &session_page,
        row,
        "_Before connecting",
        &fields.pre_command,
    );
    row += 1;
    add_row(&session_page, row, "Its timeout (s)", &fields.pre_timeout);
    row += 1;
    add_row(&session_page, row, "Wake-on-_LAN MAC", &fields.wol_mac);
    row += 1;
    add_row(&session_page, row, "Wake-on-LAN wait (s)", &fields.wol_wait);
    row += 1;
    session_page.attach(&fields.auto_reconnect, 1, row, 1, 1);
    row += 1;
    add_row(
        &session_page,
        row,
        "Reconnect after (s)",
        &fields.reconnect_delay,
    );
    row += 1;
    session_page.attach(&fields.log_output, 1, row, 1, 1);
    row += 1;
    add_row(&session_page, row, "Lo_g folder", &fields.log_dir);
    row += 1;
    session_page.attach(&fields.confirm, 1, row, 1, 1);

    // ── Macros ──
    let (macro_page, macro_list, add_macro) = list_page(
        "Add Macro",
        "Commands kept with this connection. Run them from the terminal's right-click menu \
         or the SSH menu.",
    );
    for item in &initial.macros {
        add_macro_row(&fields, &macro_list, item.clone());
    }
    {
        let fields = fields.clone();
        let list = macro_list.clone();
        add_macro.connect_clicked(move |_| add_macro_row(&fields, &list, Macro::default()));
    }

    // ── Automation ──
    let (expect_page, expect_list, add_expect) = list_page(
        "Add Rule",
        "Type a command when a pattern appears in the terminal, for example to answer a \
         banner. The server's output can trigger it, so never put a secret here. Columns: \
         pattern, command, times, delay in seconds.",
    );
    for rule in &initial.expect {
        add_expect_row(&fields, &expect_list, rule.clone());
    }
    {
        let fields = fields.clone();
        let list = expect_list.clone();
        add_expect.connect_clicked(move |_| add_expect_row(&fields, &list, ExpectRule::default()));
    }

    // ── Appearance ──
    let appearance_page = grid();
    add_row(&appearance_page, 0, "Color _scheme", &fields.color_scheme);
    add_row(&appearance_page, 1, "_Font", &fields.font);
    add_row(&appearance_page, 2, "Scroll_back lines", &fields.scrollback);
    add_row(&appearance_page, 3, "E_ncoding", &fields.encoding);

    let notebook = gtk::Notebook::new();
    notebook.set_scrollable(true);
    notebook.append_page(&connection_page, Some(&gtk::Label::new(Some("Connection"))));
    notebook.append_page(&auth_page, Some(&gtk::Label::new(Some("Authentication"))));
    notebook.append_page(&scrolled(&ssh_page), Some(&gtk::Label::new(Some("SSH"))));
    notebook.append_page(
        &scrolled(&session_page),
        Some(&gtk::Label::new(Some("Session"))),
    );
    notebook.append_page(&tunnel_page, Some(&gtk::Label::new(Some("Tunnels"))));
    notebook.append_page(
        &scrolled(&macro_page),
        Some(&gtk::Label::new(Some("Macros"))),
    );
    notebook.append_page(
        &scrolled(&expect_page),
        Some(&gtk::Label::new(Some("Automation"))),
    );
    notebook.append_page(&appearance_page, Some(&gtk::Label::new(Some("Appearance"))));

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
                            "This entry acts on this computer or by itself: {}",
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
    for button in [
        &fields.x11,
        &fields.compression,
        &fields.agent_forward,
        &fields.no_sharing,
        &fields.legacy_kex,
        &fields.legacy_cipher,
        &fields.legacy_hostkey,
        &fields.legacy_mac,
        &fields.legacy_pubkey,
        &fields.keep_shell,
    ] {
        let refresh = refresh.clone();
        button.connect_toggled(move |_| refresh());
    }
    for field in [
        &fields.keepalive,
        &fields.connect_timeout,
        &fields.pre_command,
        &fields.wol_mac,
        &fields.log_dir,
    ] {
        let refresh = refresh.clone();
        field.connect_changed(move |_| refresh());
    }
    {
        let refresh = refresh.clone();
        fields.host_key.connect_changed(move |_| refresh());
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
                let typed_secret = fields.totp_secret.text().to_string();
                let totp_secret = if connection.totp && !typed_secret.trim().is_empty() {
                    match totp::normalize_secret(&typed_secret) {
                        Ok(secret) => Some(secret),
                        Err(message) => {
                            error.set_text(&message);
                            continue;
                        }
                    }
                } else {
                    None
                };
                let typed = fields.password.text().to_string();
                let password =
                    (connection.auth == Auth::SecretService && !typed.is_empty()).then_some(typed);
                break Some(FormResult {
                    connection,
                    password,
                    totp_secret,
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
