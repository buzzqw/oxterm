//! Remote resource monitor tab of the shared right-hand dock.
//!
//! A small worker thread runs one `ssh <host> <script>` command (no local
//! shell, no secrets in argv) and posts the parsed result back to the GTK main
//! loop. Text sparklines keep the widget tree cheap: no drawing area, no
//! native dependency, just labels over the last 60 samples.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::prelude::*;

const MAX_SAMPLES: usize = 60;
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const MARKERS: [&str; 5] = ["@@CPU@@", "@@MEM@@", "@@DISK@@", "@@NET@@", "@@GPU@@"];

/// Output of `df -P` for one mounted filesystem: only the fields we show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskUsage {
    pub mount: String,
    pub used_percent: f32,
}

/// One line of `nvidia-smi --query-gpu=...`. `None` when the tool is absent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuStats {
    pub utilization: f32,
    pub used_mb: u64,
    pub total_mb: u64,
}

/// Display-free result of the remote probe. Every `parse_*` function works on
/// a plain `&str`, so it can be tested without ssh or GTK.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteStats {
    pub cpu_percent: Option<f32>,
    pub mem_total: Option<u64>,
    pub mem_used: Option<u64>,
    pub disks: Vec<DiskUsage>,
    pub net_rx_bytes: Option<u64>,
    pub net_tx_bytes: Option<u64>,
    pub gpu: Option<GpuStats>,
}

impl RemoteStats {
    /// Split the combined remote script output into its sections and parse
    /// each one independently. Missing sections simply leave a field `None`.
    pub fn parse(output: &str) -> RemoteStats {
        let [cpu, mem, disk, net, gpu] = sections(output);
        let (mem_total, mem_used) = parse_memory(mem).unzip();
        let (net_rx_bytes, net_tx_bytes) = parse_net(net).unzip();
        RemoteStats {
            cpu_percent: parse_cpu(cpu),
            mem_total,
            mem_used,
            disks: parse_disk(disk),
            net_rx_bytes,
            net_tx_bytes,
            gpu: parse_gpu(gpu),
        }
    }
}

/// CPU usage from the summary line of `top -bn1` (100 - idle).
pub fn parse_cpu(top: &str) -> Option<f32> {
    for line in top.lines() {
        let Some(pos) = line.find("%Cpu(s):") else {
            continue;
        };
        for field in line[pos + "%Cpu(s):".len()..].split(',') {
            let field = field.trim();
            if !field.ends_with("id") {
                continue;
            }
            if let Some(idle) = field
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<f32>().ok())
            {
                return Some((100.0 - idle).clamp(0.0, 100.0));
            }
        }
    }
    None
}

/// `(total, used)` in bytes from the `Mem:` line of `free -b`.
pub fn parse_memory(free: &str) -> Option<(u64, u64)> {
    for line in free.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("Mem:") {
            continue;
        }
        let total = fields.next()?.parse::<u64>().ok()?;
        let used = fields.next()?.parse::<u64>().ok()?;
        return Some((total, used));
    }
    None
}

/// Used percentage and mount point from every data line of `df -P`.
pub fn parse_disk(df: &str) -> Vec<DiskUsage> {
    let mut disks = Vec::new();
    for line in df.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 || fields[0] == "Filesystem" {
            continue;
        }
        if let Ok(used_percent) = fields[4].trim_end_matches('%').parse::<f32>() {
            disks.push(DiskUsage {
                mount: fields[5].to_string(),
                used_percent,
            });
        }
    }
    disks
}

/// `(received, transmitted)` bytes from `/proc/net/dev`, excluding `lo`.
pub fn parse_net(dev: &str) -> Option<(u64, u64)> {
    let (mut rx, mut tx) = (0u64, 0u64);
    let mut found = false;
    for line in dev.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        let (Some(r), Some(t)) = (fields[0].parse::<u64>().ok(), fields[8].parse::<u64>().ok())
        else {
            continue;
        };
        rx = rx.saturating_add(r);
        tx = tx.saturating_add(t);
        found = true;
    }
    found.then_some((rx, tx))
}

/// First GPU row of `nvidia-smi --query-gpu=... --format=csv,noheader`.
pub fn parse_gpu(text: &str) -> Option<GpuStats> {
    let line = text.lines().find(|line| !line.trim().is_empty())?.trim();
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    if fields.len() < 3 {
        return None;
    }
    let utilization = fields[0].trim_end_matches('%').parse::<f32>().ok()?;
    let used_mb = fields[1].split_whitespace().next()?.parse::<u64>().ok()?;
    let total_mb = fields[2].split_whitespace().next()?.parse::<u64>().ok()?;
    Some(GpuStats {
        utilization,
        used_mb,
        total_mb,
    })
}

/// Map 0..=100 values onto the eight Unicode block glyphs.
pub fn sparkline(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| {
            let index =
                ((value.clamp(0.0, 100.0) / 100.0) * (BARS.len() - 1) as f32).round() as usize;
            BARS[index]
        })
        .collect()
}

fn sections(output: &str) -> [&str; 5] {
    let mut found = ["", "", "", "", ""];
    let mut rest = output;
    for (index, marker) in MARKERS.iter().enumerate() {
        let Some(pos) = rest.find(marker) else {
            break;
        };
        let after = &rest[pos + marker.len()..];
        let end = MARKERS
            .get(index + 1)
            .and_then(|next| after.find(next))
            .unwrap_or(after.len());
        found[index] = &after[..end];
        rest = &after[end..];
    }
    found
}

/// The exact remote script. `timeout 5` bounds a whole probe; the GPU query is
/// optional so hosts without nvidia-smi still report the other metrics.
const PROBE_SCRIPT: &str = "timeout 5 sh -c 'echo @@CPU@@; top -bn1 | head -n 5; \
     echo @@MEM@@; free -b; echo @@DISK@@; df -P; echo @@NET@@; cat /proc/net/dev; \
     echo @@GPU@@; nvidia-smi --query-gpu=utilization.gpu,memory.used,memory.total \
     --format=csv,noheader 2>/dev/null || true'";

fn probe_command(host: &str) -> Result<Command, String> {
    if host.trim().is_empty() {
        // The entry says an empty target means localhost. Probe this machine
        // directly: requiring sshd on localhost made the tooltip a lie.
        let mut command = Command::new("sh");
        command.arg("-c").arg(PROBE_SCRIPT);
        return Ok(command);
    }
    // `user@host:2223` is a display string, not an ssh destination: ssh would
    // try to resolve `host:2223` as a hostname. Split it into -l/-p.
    let args = crate::headless::connections::ssh_target_args(host)
        .map_err(|error| format!("{error} (es. user@server:2222)"))?;
    let mut command = Command::new("ssh");
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .args(args)
        .arg(PROBE_SCRIPT);
    Ok(command)
}

fn run_remote_stats(host: &str) -> Result<RemoteStats, String> {
    let output = probe_command(host)?
        .output()
        .map_err(|error| format!("Impossibile avviare il comando: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("ssh terminato con {status}", status = output.status)
        } else {
            detail
        });
    }
    Ok(RemoteStats::parse(&String::from_utf8_lossy(&output.stdout)))
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    cpu: f32,
    ram: f32,
    net: f32,
    gpu: Option<GpuStats>,
}

struct State {
    stop: Arc<AtomicBool>,
    generation: u64,
    host: String,
    samples: Vec<Sample>,
    last_net: Option<(Instant, u64, u64)>,
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
    gpu: crate::SendWeak<gtk::Label>,
}

impl Ui {
    fn new(
        status: &gtk::Label,
        cpu: &gtk::Label,
        ram: &gtk::Label,
        disk: &gtk::Label,
        net: &gtk::Label,
        gpu: &gtk::Label,
    ) -> Ui {
        Ui {
            status: crate::SendWeak::new(status),
            cpu: crate::SendWeak::new(cpu),
            ram: crate::SendWeak::new(ram),
            disk: crate::SendWeak::new(disk),
            net: crate::SendWeak::new(net),
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
                    format!("{} / {} MiB", used / 1024 / 1024, total / 1024 / 1024),
                ),
                _ => (0.0, "n/d".to_string()),
            };
            let (rx, tx) = (
                stats.net_rx_bytes.unwrap_or(0),
                stats.net_tx_bytes.unwrap_or(0),
            );
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
                "<tt>RAM  {:5.1}%  {}</tt>",
                ram_percent,
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
            ui.net.set_markup(&format!(
                "<tt>Net   {:7.1} KiB/s  {}</tt>",
                net_kbps,
                column(|sample| sample.net)
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
    pub gpu: gtk::Label,
    state: Arc<Mutex<State>>,
}

impl MonitorPanel {
    pub fn build() -> MonitorPanel {
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

        let status = gtk::Label::new(Some("Premi Start per campionare le risorse remote"));
        status.set_xalign(0.0);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_margin_start(8);
        status.set_margin_end(8);
        status.set_margin_top(4);
        status.set_margin_bottom(6);
        status.style_context().add_class("dim-label");
        root.pack_start(&status, false, false, 0);

        let state = Arc::new(Mutex::new(State {
            stop: Arc::new(AtomicBool::new(false)),
            generation: 0,
            host: String::new(),
            samples: Vec::new(),
            last_net: None,
        }));
        let ui = Ui::new(&status, &cpu, &ram, &disk, &net, &gpu);

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

        MonitorPanel {
            root,
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
            gpu,
            state,
        }
    }

    /// Prefill the target from the active SSH connection, if any. Kept as a
    /// method so the window can decide when the value is worth overriding.
    pub fn set_target(&self, target: Option<String>) {
        if let Some(target) = target.filter(|target| !target.is_empty()) {
            self.host.set_text(&target);
        }
    }

    /// Cancel the worker, e.g. when the dock is hidden.
    pub fn pause(&self) {
        stop_poll(&self.state);
        self.start.set_sensitive(true);
        self.stop.set_sensitive(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn probe_splits_the_port_and_probes_localhost_when_empty() {
        assert_eq!(
            argv(&probe_command("andres@192.168.1.161:2223").unwrap()),
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-l",
                "andres",
                "-p",
                "2223",
                "192.168.1.161",
                PROBE_SCRIPT
            ]
        );
        assert_eq!(argv(&probe_command("").unwrap()), ["-c", PROBE_SCRIPT]);
        assert!(probe_command("-oProxyCommand=evil").is_err());
    }

    #[test]
    fn cpu_is_one_minus_idle() {
        let top = "top - 12:00:00 up 1 day\nTasks: 1 total\n\
                   %Cpu(s):  5.9 us,  1.9 sy,  0.0 ni, 91.9 id,  0.2 wa,  0.0 hi\n";
        assert!((parse_cpu(top).unwrap() - 8.1).abs() < 0.001);
        assert_eq!(parse_cpu("no summary here"), None);
    }

    #[test]
    fn memory_reads_total_and_used() {
        let free = "              total        used        free      shared  buff/cache   available\n\
                    Mem:        1048576000   419430400   629145600           0     209715200     800000000\n\
                    Swap:                 0           0           0\n";
        assert_eq!(parse_memory(free), Some((1048576000, 419430400)));
        assert_eq!(parse_memory("Swap: 1 2 3"), None);
    }

    #[test]
    fn disk_keeps_mount_and_capacity() {
        let df = "Filesystem 1024-blocks      Used Available Capacity Mounted on\n\
                  /dev/sda1    10240000   4194304   6045696      41% /\n\
                  tmpfs           1024         0      1024       0% /run\n\
                  bad line\n";
        assert_eq!(
            parse_disk(df),
            vec![
                DiskUsage {
                    mount: "/".to_string(),
                    used_percent: 41.0,
                },
                DiskUsage {
                    mount: "/run".to_string(),
                    used_percent: 0.0,
                },
            ]
        );
    }

    #[test]
    fn net_sums_interfaces_without_loopback() {
        let dev = "Inter-|   Receive                                                |  Transmit\n\
                    lo: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0\n\
                  eth0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n\
                  eth1: 500 5 0 0 0 0 0 0 700 7 0 0 0 0 0 0\n";
        assert_eq!(parse_net(dev), Some((1500, 2700)));
        assert_eq!(parse_net("only a header"), None);
    }

    #[test]
    fn gpu_parses_csv_and_tolerates_missing_tool() {
        let csv = "45, 2048, 8192\n";
        assert_eq!(
            parse_gpu(csv),
            Some(GpuStats {
                utilization: 45.0,
                used_mb: 2048,
                total_mb: 8192,
            })
        );
        assert_eq!(parse_gpu(""), None);
    }

    #[test]
    fn combined_output_is_split_into_sections() {
        let output = "@@CPU@@\n%Cpu(s): 10.0 id\n@@MEM@@\nMem: 100 40 60 0 0 0\n\
                      @@DISK@@\nFilesystem 1024-blockes Used Avail Capacity Mounted on\n/dev/sda1 10 4 6 40% /\n\
                      @@NET@@\n eth0: 10 1 0 0 0 0 0 0 20 2 0 0 0 0 0 0\n@@GPU@@\n\n";
        let stats = RemoteStats::parse(output);
        assert_eq!(stats.cpu_percent, Some(90.0));
        assert_eq!(stats.mem_used, Some(40));
        assert_eq!(stats.disks.len(), 1);
        assert_eq!(stats.net_tx_bytes, Some(20));
        assert_eq!(stats.gpu, None);
    }

    #[test]
    fn sparkline_maps_extremes_and_keeps_order() {
        assert_eq!(sparkline(&[0.0, 100.0]), "▁█");
        assert_eq!(sparkline(&[]).len(), 0);
        let line = sparkline(&vec![50.0; 60]);
        assert_eq!(line.chars().count(), 60);
        assert!(line.chars().all(|glyph| BARS.contains(&glyph)));
    }
}
