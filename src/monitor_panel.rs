//! Remote resource monitor tab of the shared right-hand dock.
//!
//! A small worker thread runs one `ssh <host> <script>` command (no local
//! shell, no secrets in argv) and posts the parsed result back to the GTK main
//! loop. Text sparklines keep the widget tree cheap: no drawing area, no
//! native dependency, just labels over the last 60 samples.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::prelude::*;

use crate::headless::monitor::{
    net_counters, run_remote_stats, sparkline, GpuStats, RemoteStats, ALL_INTERFACES,
};

const MAX_SAMPLES: usize = 60;

#[derive(Clone, Debug)]
struct Sample {
    cpu: f32,
    ram: f32,
    net: f32,
    /// Interface this rate belongs to; `None` means the summed "all" view.
    net_iface: Option<String>,
    gpu: Option<GpuStats>,
}

struct State {
    stop: Arc<AtomicBool>,
    generation: u64,
    host: String,
    samples: Vec<Sample>,
    last_net: Option<(Instant, u64, u64)>,
    /// Selected interface for the Net row; `None` sums every non-loopback one.
    net_interface: Option<String>,
    /// Set while `apply()` rebuilds the selector, so programmatic changes do
    /// not reset the counters or fire a user-driven refresh.
    syncing_net: Arc<AtomicBool>,
}

enum PollResult {
    Stats(RemoteStats),
    Error(String),
}

#[derive(Clone)]
struct Ui {
    status: crate::SendWeak<gtk::Label>,
    cpu: crate::SendWeak<gtk::Label>,
    ram: crate::SendWeak<gtk::Label>,
    disk: crate::SendWeak<gtk::Label>,
    net: crate::SendWeak<gtk::Label>,
    net_iface: crate::SendWeak<gtk::ComboBoxText>,
    gpu: crate::SendWeak<gtk::Label>,
}

impl Ui {
    fn new(
        status: &gtk::Label,
        cpu: &gtk::Label,
        ram: &gtk::Label,
        disk: &gtk::Label,
        net: &gtk::Label,
        net_iface: &gtk::ComboBoxText,
        gpu: &gtk::Label,
    ) -> Ui {
        Ui {
            status: crate::SendWeak::new(status),
            cpu: crate::SendWeak::new(cpu),
            ram: crate::SendWeak::new(ram),
            disk: crate::SendWeak::new(disk),
            net: crate::SendWeak::new(net),
            net_iface: crate::SendWeak::new(net_iface),
            gpu: crate::SendWeak::new(gpu),
        }
    }

    fn upgrade(&self) -> Option<StrongUi> {
        Some(StrongUi {
            status: self.status.upgrade()?,
            cpu: self.cpu.upgrade()?,
            ram: self.ram.upgrade()?,
            disk: self.disk.upgrade()?,
            net: self.net.upgrade()?,
            net_iface: self.net_iface.upgrade()?,
            gpu: self.gpu.upgrade()?,
        })
    }
}

struct StrongUi {
    status: gtk::Label,
    cpu: gtk::Label,
    ram: gtk::Label,
    disk: gtk::Label,
    net: gtk::Label,
    net_iface: gtk::ComboBoxText,
    gpu: gtk::Label,
}

fn spawn_poll(state: &Arc<Mutex<State>>, ui: &Ui, host: String, interval: u64, once: bool) {
    let (generation, stop) = {
        let mut state = state.lock().expect("monitor state lock poisoned");
        state.generation = state.generation.wrapping_add(1);
        state.stop.store(false, Ordering::SeqCst);
        if state.host != host {
            // Counters from a different machine are not comparable.
            state.host = host.clone();
            state.samples.clear();
            state.last_net = None;
        }
        (state.generation, state.stop.clone())
    };

    let shared = state.clone();
    let worker_ui = ui.clone();
    std::thread::spawn(move || loop {
        if stop.load(Ordering::SeqCst)
            || shared
                .lock()
                .expect("monitor state lock poisoned")
                .generation
                != generation
        {
            break;
        }
        let result = match run_remote_stats(&host) {
            Ok(stats) => PollResult::Stats(stats),
            Err(message) => PollResult::Error(message),
        };
        let state = shared.clone();
        let ui = worker_ui.clone();
        crate::run_on_main(move || {
            let mut state = state.lock().expect("monitor state lock poisoned");
            if state.generation != generation {
                return; // a newer Start/Refresh/Stop superseded this result
            }
            if let Some(ui) = ui.upgrade() {
                apply(&ui, &mut state, result);
            }
        });
        if once {
            break;
        }
        // Sleep in small slices so Stop is responsive even at 60 s intervals.
        for _ in 0..interval.saturating_mul(10) {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}

fn stop_poll(state: &Arc<Mutex<State>>) {
    let mut state = state.lock().expect("monitor state lock poisoned");
    state.generation = state.generation.wrapping_add(1);
    state.stop.store(true, Ordering::SeqCst);
}

fn apply(ui: &StrongUi, state: &mut State, result: PollResult) {
    match result {
        PollResult::Error(message) => ui.status.set_text(&format!("Errore: {message}")),
        PollResult::Stats(stats) => {
            let cpu = stats.cpu_percent.unwrap_or(0.0);
            let (ram_percent, ram_text) = match (stats.mem_total, stats.mem_used) {
                (Some(total), Some(used)) if total > 0 => (
                    used as f32 * 100.0 / total as f32,
                    format!("{}/{}MiB", used / 1024 / 1024, total / 1024 / 1024),
                ),
                _ => (0.0, "n/d".to_string()),
            };

            // The interfaces exist on the target, so the list is rebuilt from
            // every probe (localhost and SSH alike). A vanished selection
            // falls back to the summed view rather than showing stale data.
            let names: Vec<String> = stats
                .net_interfaces
                .iter()
                .map(|interface| interface.name.clone())
                .collect();
            let wanted = state.net_interface.clone();
            let active = match &wanted {
                None => 0,
                Some(name) => names
                    .iter()
                    .position(|candidate| candidate == name)
                    .map(|index| index + 1)
                    .unwrap_or(0),
            };
            if active == 0 && wanted.is_some() {
                state.net_interface = None;
                state.last_net = None;
            }
            let net_interface = state.net_interface.clone();

            state.syncing_net.store(true, Ordering::SeqCst);
            ui.net_iface.remove_all();
            ui.net_iface.append_text(ALL_INTERFACES);
            for name in &names {
                ui.net_iface.append_text(name);
            }
            ui.net_iface.set_active(Some(active as u32));
            state.syncing_net.store(false, Ordering::SeqCst);

            let (rx, tx) = net_counters(&stats.net_interfaces, net_interface.as_deref());
            let now = Instant::now();
            let net_kbps = match state.last_net {
                Some((previous, previous_rx, previous_tx)) => {
                    let seconds = now.duration_since(previous).as_secs_f32().max(0.001);
                    let delta = rx.saturating_sub(previous_rx) + tx.saturating_sub(previous_tx);
                    delta as f32 / seconds / 1024.0
                }
                None => 0.0,
            };
            state.last_net = Some((now, rx, tx));
            state.samples.push(Sample {
                cpu,
                ram: ram_percent,
                net: net_kbps.min(100.0),
                net_iface: net_interface.clone(),
                gpu: stats.gpu,
            });
            if state.samples.len() > MAX_SAMPLES {
                state.samples.remove(0);
            }

            let column = |field: fn(&Sample) -> f32| -> String {
                sparkline(&state.samples.iter().map(field).collect::<Vec<f32>>())
            };
            ui.cpu.set_markup(&format!(
                "<tt>CPU  {:5.1}%  {}</tt>",
                cpu,
                column(|sample| sample.cpu)
            ));
            ui.ram.set_markup(&format!(
                "<tt>RAM  {:5.1}%  {:<15} {}</tt>",
                ram_percent,
                ram_text,
                column(|sample| sample.ram)
            ));
            let disk_text = if stats.disks.is_empty() {
                "n/d".to_string()
            } else {
                stats
                    .disks
                    .iter()
                    .take(3)
                    .map(|disk| format!("{} {:.0}%", disk.mount, disk.used_percent))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            ui.disk.set_markup(&format!(
                "<tt>Disk  {}</tt>",
                glib::markup_escape_text(&disk_text)
            ));
            // Only rates sampled while this interface was selected feed the
            // trend, otherwise switching would mix incomparable counters.
            let net_trend: Vec<f32> = state
                .samples
                .iter()
                .filter(|sample| sample.net_iface == net_interface)
                .map(|sample| sample.net)
                .collect();
            let iface_text = net_interface.unwrap_or_else(|| "tutte".to_string());
            ui.net.set_markup(&format!(
                "<tt>Net {} {:7.1} KiB/s  {}</tt>",
                glib::markup_escape_text(&iface_text),
                net_kbps,
                sparkline(&net_trend)
            ));
            let gpu_text = match stats.gpu {
                Some(gpu) => {
                    let trend: Vec<f32> = state
                        .samples
                        .iter()
                        .filter_map(|sample| sample.gpu.map(|value| value.utilization))
                        .collect();
                    format!(
                        "GPU  {:.0}%  {}/{} MiB  {}",
                        gpu.utilization,
                        gpu.used_mb,
                        gpu.total_mb,
                        sparkline(&trend)
                    )
                }
                None => "GPU  n/d (nvidia-smi assente)".to_string(),
            };
            ui.gpu
                .set_markup(&format!("<tt>{}</tt>", glib::markup_escape_text(&gpu_text)));
            ui.status.set_text(&format!(
                "Live · {} campioni · RAM {ram_text}",
                state.samples.len()
            ));
        }
    }
}

/// Public state of the Monitor tab. `build()` wires all signals, so callers
/// only need to keep the returned value alive and connect `close` if wanted.
pub struct MonitorPanel {
    pub root: gtk::Box,
    /// Dropdown of saved SSH connections (active ones first). Selecting an
    /// entry fills `host`; typing in `host` still works for arbitrary targets.
    pub connections: gtk::ComboBoxText,
    pub host: gtk::Entry,
    pub interval: gtk::SpinButton,
    pub start: gtk::Button,
    pub stop: gtk::Button,
    pub refresh: gtk::Button,
    pub close: gtk::Button,
    pub status: gtk::Label,
    pub cpu: gtk::Label,
    pub ram: gtk::Label,
    pub disk: gtk::Label,
    pub net: gtk::Label,
    /// Interface used by the Net row; populated by every probe.
    pub net_iface: gtk::ComboBoxText,
    pub gpu: gtk::Label,
    state: Arc<Mutex<State>>,
    /// Target behind each entry of `connections` (index-aligned).
    connection_targets: RefCell<Vec<String>>,
    syncing_connections: Arc<AtomicBool>,
}

impl MonitorPanel {
    /// `connection_choices` is `(target, label, active)`; active SSH
    /// connections are passed first and keep that order in the dropdown.
    pub fn build(connection_choices: Vec<(String, String, bool)>) -> MonitorPanel {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_width_request(220);
        root.set_no_show_all(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_top(6);
        header.set_margin_bottom(4);
        header.set_margin_start(8);
        header.set_margin_end(8);
        let title = gtk::Label::new(None);
        title.set_markup("<b>Monitor</b>");
        title.set_halign(gtk::Align::Start);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        header.pack_start(&title, true, true, 0);
        let close = gtk::Button::with_label("Close");
        close.set_tooltip_text(Some("Hide the shared dock"));
        header.pack_end(&close, false, false, 0);
        root.pack_start(&header, false, false, 0);

        let conn_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        conn_row.set_margin_start(8);
        conn_row.set_margin_end(8);
        let conn_label = gtk::Label::new(Some("Conn."));
        conn_label.set_xalign(0.0);
        let connections = gtk::ComboBoxText::new();
        connections.set_hexpand(true);
        connections.set_tooltip_text(Some(
            "Connessioni SSH salvate (prime quelle attive); oppure scrivi un target qui sotto",
        ));
        let mut connection_targets = Vec::new();
        for (target, label, active) in &connection_choices {
            let text = if *active {
                format!("● {label}")
            } else {
                label.clone()
            };
            connections.append_text(&text);
            connection_targets.push(target.clone());
        }
        // Localhost is not one of the saved connections: keep it below them,
        // while the active SSH sessions stay at the top as requested.
        connections.append_text("localhost (questa macchina)");
        connection_targets.push(String::new());
        connections.set_active(Some((connection_targets.len() - 1) as u32));
        conn_row.pack_start(&conn_label, false, false, 0);
        conn_row.pack_start(&connections, true, true, 0);
        root.pack_start(&conn_row, false, false, 0);

        let target_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        target_row.set_margin_start(8);
        target_row.set_margin_end(8);
        let host = gtk::Entry::new();
        host.set_hexpand(true);
        host.set_placeholder_text(Some("user@host[:port] (vuoto = localhost)"));
        let interval = gtk::SpinButton::new(
            Some(&gtk::Adjustment::new(2.0, 1.0, 60.0, 1.0, 10.0, 0.0)),
            0.0,
            0,
        );
        interval.set_numeric(true);
        interval.set_width_chars(3);
        interval.set_tooltip_text(Some("Refresh interval in seconds (1-60)"));
        let interval_label = gtk::Label::new(Some("s"));
        target_row.pack_start(&host, true, true, 0);
        target_row.pack_end(&interval, false, false, 0);
        target_row.pack_end(&interval_label, false, false, 0);
        root.pack_start(&target_row, false, false, 0);

        let syncing_connections = Arc::new(AtomicBool::new(false));
        let conn_sync = syncing_connections.clone();
        let conn_host = host.clone();
        let conn_targets = Arc::new(connection_targets.clone());
        connections.connect_changed(move |combo| {
            if conn_sync.load(Ordering::SeqCst) {
                return;
            }
            if let Some(index) = combo.active() {
                if let Some(target) = conn_targets.get(index as usize) {
                    conn_host.set_text(target);
                }
            }
        });

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_margin_start(8);
        actions.set_margin_end(8);
        actions.set_margin_top(4);
        let start = gtk::Button::with_label("Start");
        let stop = gtk::Button::with_label("Stop");
        let refresh = gtk::Button::with_label("Refresh");
        stop.set_sensitive(false);
        actions.pack_start(&start, false, false, 0);
        actions.pack_start(&stop, false, false, 0);
        actions.pack_start(&refresh, false, false, 0);
        root.pack_start(&actions, false, false, 0);

        let metrics = gtk::Box::new(gtk::Orientation::Vertical, 2);
        metrics.set_margin_start(8);
        metrics.set_margin_end(8);
        metrics.set_margin_top(6);
        let cpu = gtk::Label::new(Some("<tt>CPU  --</tt>"));
        let ram = gtk::Label::new(Some("<tt>RAM  --</tt>"));
        let disk = gtk::Label::new(Some("<tt>Disk  --</tt>"));
        let net = gtk::Label::new(Some("<tt>Net   --</tt>"));
        let gpu = gtk::Label::new(Some("<tt>GPU  --</tt>"));
        for label in [&cpu, &ram, &disk, &net, &gpu] {
            label.set_use_markup(true);
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            metrics.pack_start(label, false, false, 0);
        }
        root.pack_start(&metrics, false, false, 0);

        let net_iface_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        net_iface_row.set_margin_start(8);
        net_iface_row.set_margin_end(8);
        net_iface_row.set_margin_top(2);
        let net_iface_label = gtk::Label::new(Some("Scheda"));
        net_iface_label.set_xalign(0.0);
        let net_iface = gtk::ComboBoxText::new();
        net_iface.set_hexpand(true);
        net_iface.append_text(ALL_INTERFACES);
        net_iface.set_active(Some(0));
        net_iface.set_tooltip_text(Some("Scheda di rete usata dalla riga Net"));
        net_iface_row.pack_start(&net_iface_label, false, false, 0);
        net_iface_row.pack_start(&net_iface, true, true, 0);
        root.pack_start(&net_iface_row, false, false, 0);

        let status = gtk::Label::new(Some("Premi Start per campionare le risorse remote"));
        status.set_xalign(0.0);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_margin_start(8);
        status.set_margin_end(8);
        status.set_margin_top(4);
        status.set_margin_bottom(6);
        status.style_context().add_class("dim-label");
        root.pack_start(&status, false, false, 0);

        let syncing_net = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(State {
            stop: Arc::new(AtomicBool::new(false)),
            generation: 0,
            host: String::new(),
            samples: Vec::new(),
            last_net: None,
            net_interface: None,
            syncing_net: syncing_net.clone(),
        }));
        let ui = Ui::new(&status, &cpu, &ram, &disk, &net, &net_iface, &gpu);

        let start_state = state.clone();
        let start_ui = ui.clone();
        let start_host = host.clone();
        let start_interval = interval.clone();
        let weak_start = start.downgrade();
        let weak_stop = stop.downgrade();
        start.connect_clicked(move |_| {
            spawn_poll(
                &start_state,
                &start_ui,
                start_host.text().to_string(),
                start_interval.value_as_int().clamp(1, 60) as u64,
                false,
            );
            let (Some(start), Some(stop)) = (weak_start.upgrade(), weak_stop.upgrade()) else {
                return;
            };
            start.set_sensitive(false);
            stop.set_sensitive(true);
        });

        let stop_state = state.clone();
        let weak_start = start.downgrade();
        let weak_stop = stop.downgrade();
        let weak_status = status.downgrade();
        stop.connect_clicked(move |_| {
            stop_poll(&stop_state);
            if let Some(status) = weak_status.upgrade() {
                status.set_text("Monitor fermato");
            }
            let (Some(start), Some(stop)) = (weak_start.upgrade(), weak_stop.upgrade()) else {
                return;
            };
            start.set_sensitive(true);
            stop.set_sensitive(false);
        });

        let refresh_state = state.clone();
        let refresh_ui = ui.clone();
        let refresh_host = host.clone();
        let refresh_interval = interval.clone();
        let refresh_status = status.clone();
        refresh.connect_clicked(move |_| {
            refresh_status.set_text("Aggiornamento...");
            spawn_poll(
                &refresh_state,
                &refresh_ui,
                refresh_host.text().to_string(),
                refresh_interval.value_as_int().clamp(1, 60) as u64,
                true,
            );
        });

        let activate_state = state.clone();
        let activate_ui = ui.clone();
        let activate_host = host.clone();
        let activate_interval = interval.clone();
        let activate_status = status.clone();
        host.connect_activate(move |_| {
            activate_status.set_text("Aggiornamento...");
            spawn_poll(
                &activate_state,
                &activate_ui,
                activate_host.text().to_string(),
                activate_interval.value_as_int().clamp(1, 60) as u64,
                true,
            );
        });

        let net_state = state.clone();
        let net_sync = syncing_net.clone();
        let net_refresh = refresh.downgrade();
        net_iface.connect_changed(move |combo| {
            // `apply()` rebuilds the list on every probe; those programmatic
            // changes must not look like a user picking a different card.
            if net_sync.load(Ordering::SeqCst) {
                return;
            }
            let selection = match combo.active_text() {
                Some(text) if text.as_str() != ALL_INTERFACES => Some(text.to_string()),
                _ => None,
            };
            let mut state = net_state.lock().expect("monitor state lock poisoned");
            if state.net_interface != selection {
                state.net_interface = selection;
                // Counters of different interfaces are not comparable.
                state.last_net = None;
                drop(state);
                if let Some(refresh) = net_refresh.upgrade() {
                    refresh.activate();
                }
            }
        });

        MonitorPanel {
            root,
            connections,
            host,
            interval,
            start,
            stop,
            refresh,
            close,
            status,
            cpu,
            ram,
            disk,
            net,
            net_iface,
            gpu,
            state,
            connection_targets: RefCell::new(connection_targets),
            syncing_connections,
        }
    }

    /// Prefill the target from the active SSH connection, if any. Kept as a
    /// method so the window can decide when the value is worth overriding.
    /// The dropdown follows the entry when the target is one of its choices.
    pub fn set_target(&self, target: Option<String>) {
        if let Some(target) = target.filter(|target| !target.is_empty()) {
            self.host.set_text(&target);
        }
        let text = self.host.text();
        let targets = self.connection_targets.borrow();
        let index = targets
            .iter()
            .position(|candidate| candidate == text.as_str())
            .unwrap_or_else(|| targets.len() - 1);
        // Keep a typed target that is not in the book: the callback would
        // otherwise overwrite the entry with the fallback localhost value.
        self.syncing_connections.store(true, Ordering::SeqCst);
        self.connections.set_active(Some(index as u32));
        self.syncing_connections.store(false, Ordering::SeqCst);
    }

    /// Refresh the saved-SSH dropdown (e.g. when the dock is shown again)
    /// without losing a target the user typed by hand.
    pub fn set_connections(&self, choices: Vec<(String, String, bool)>) {
        let current = self.host.text().to_string();
        let mut targets = Vec::new();
        self.syncing_connections.store(true, Ordering::SeqCst);
        self.connections.remove_all();
        for (target, label, active) in &choices {
            let text = if *active {
                format!("● {label}")
            } else {
                label.clone()
            };
            self.connections.append_text(&text);
            targets.push(target.clone());
        }
        self.connections.append_text("localhost (questa macchina)");
        targets.push(String::new());
        let index = targets
            .iter()
            .position(|target| target == &current)
            .unwrap_or(targets.len() - 1);
        self.connections.set_active(Some(index as u32));
        self.syncing_connections.store(false, Ordering::SeqCst);
        *self.connection_targets.borrow_mut() = targets;
    }

    /// Cancel the worker, e.g. when the dock is hidden.
    pub fn pause(&self) {
        stop_poll(&self.state);
        self.start.set_sensitive(true);
        self.stop.set_sensitive(false);
    }
}
