//! Display-free half of the resource monitor: build one shell/ssh probe and
//! turn its output into the values shown by the GTK tab. No GTK here, so the
//! parsers are covered by the headless tests as well as by unit tests.

use std::process::Command;

use super::connections::ssh_target_args;

const MARKERS: [&str; 5] = ["@@CPU@@", "@@MEM@@", "@@DISK@@", "@@NET@@", "@@GPU@@"];
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// First entry of the network selector: the old behaviour, every interface
/// except the loopback summed together.
pub const ALL_INTERFACES: &str = "Tutte (eccetto lo)";

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

/// One line of `/proc/net/dev` (loopback excluded).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetInterface {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Display-free result of the remote probe. Every `parse_*` function works on
/// a plain `&str`, so it can be tested without ssh or GTK.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteStats {
    pub cpu_percent: Option<f32>,
    pub mem_total: Option<u64>,
    pub mem_used: Option<u64>,
    pub disks: Vec<DiskUsage>,
    pub net_interfaces: Vec<NetInterface>,
    pub gpu: Option<GpuStats>,
}

impl RemoteStats {
    /// Split the combined remote script output into its sections and parse
    /// each one independently. Missing sections simply leave a field `None`.
    pub fn parse(output: &str) -> RemoteStats {
        let [cpu, mem, disk, net, gpu] = sections(output);
        let (mem_total, mem_used) = parse_memory(mem).unzip();
        RemoteStats {
            cpu_percent: parse_cpu(cpu),
            mem_total,
            mem_used,
            disks: parse_disk(disk),
            net_interfaces: parse_net(net),
            gpu: parse_gpu(gpu),
        }
    }
}

/// `top` prints a comma as decimal separator under locale settings such as
/// `it_IT`, so a plain `parse::<f32>()` would throw away the real value.
fn parse_decimal(value: &str) -> Option<f32> {
    value.replace(',', ".").parse::<f32>().ok()
}

/// CPU usage from the summary line of `top -bn1` (100 - idle).
///
/// The idle field is found by its `id` label, not by splitting on commas:
/// `95,6 id` would otherwise be split into `95` and `6 id`, making an idle
/// machine look 94% busy.
pub fn parse_cpu(top: &str) -> Option<f32> {
    for line in top.lines() {
        let Some(pos) = line.find("%Cpu(s):") else {
            continue;
        };
        let fields: Vec<&str> = line[pos + "%Cpu(s):".len()..].split_whitespace().collect();
        for pair in fields.windows(2) {
            if pair[1].trim_end_matches(',') != "id" {
                continue;
            }
            if let Some(idle) = parse_decimal(pair[0]) {
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
        if fields.len() < 6 || fields[0].starts_with("Filesystem") || fields[0].starts_with("File")
        {
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

/// Byte counters of every interface in `/proc/net/dev`, excluding `lo`.
pub fn parse_net(dev: &str) -> Vec<NetInterface> {
    let mut interfaces = Vec::new();
    for line in dev.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name == "lo" {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        let (Some(rx), Some(tx)) = (fields[0].parse::<u64>().ok(), fields[8].parse::<u64>().ok())
        else {
            continue;
        };
        interfaces.push(NetInterface {
            name: name.to_string(),
            rx_bytes: rx,
            tx_bytes: tx,
        });
    }
    interfaces
}

/// `(received, transmitted)` counters for the selected interface. `None`
/// means "all interfaces" and sums them, as the monitor did before the
/// selector existed.
pub fn net_counters(interfaces: &[NetInterface], selection: Option<&str>) -> (u64, u64) {
    match selection {
        None => interfaces.iter().fold((0u64, 0u64), |(rx, tx), interface| {
            (
                rx.saturating_add(interface.rx_bytes),
                tx.saturating_add(interface.tx_bytes),
            )
        }),
        Some(name) => interfaces
            .iter()
            .find(|interface| interface.name == name)
            .map(|interface| (interface.rx_bytes, interface.tx_bytes))
            .unwrap_or((0, 0)),
    }
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

/// Two spellings of an SSH destination can point at the same machine
/// (`user@host:2222` in the book, `host` from a live client). Compare hosts
/// only, so the Monitor dropdown can mark open connections as active.
pub fn same_ssh_host(a: &str, b: &str) -> bool {
    fn host(target: &str) -> &str {
        let host = target.rsplit('@').next().unwrap_or(target);
        host.split_once(':').map(|(name, _)| name).unwrap_or(host)
    }
    !a.trim().is_empty() && !b.trim().is_empty() && host(a) == host(b)
}

/// Stable sort that puts active SSH connections at the top of the dropdown.
pub fn order_connection_choices(
    mut choices: Vec<(String, String, bool)>,
) -> Vec<(String, String, bool)> {
    choices.sort_by_key(|choice| !choice.2);
    choices
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

/// The exact remote script. `LC_ALL=C` keeps `top`, `free` and `df` from using
/// locale decimal separators, which used to invert the CPU reading. `timeout 5`
/// bounds a whole probe; the GPU query is optional so hosts without nvidia-smi
/// still report the other metrics.
pub const PROBE_SCRIPT: &str = "LC_ALL=C timeout 5 sh -c 'echo @@CPU@@; top -bn1 | head -n 5; \
     echo @@MEM@@; free -b; echo @@DISK@@; df -P; echo @@NET@@; cat /proc/net/dev; \
     echo @@GPU@@; nvidia-smi --query-gpu=utilization.gpu,memory.used,memory.total \
     --format=csv,noheader 2>/dev/null || true'";

/// Build the probe command. An empty target means this machine (no sshd
/// needed); a remote target is split into `-l`/`-p` so `user@host:2223` is not
/// resolved as a hostname.
pub fn probe_command(host: &str) -> Result<Command, String> {
    if host.trim().is_empty() {
        let mut command = Command::new("sh");
        command.arg("-c").arg(PROBE_SCRIPT);
        return Ok(command);
    }
    let args = ssh_target_args(host).map_err(|error| format!("{error} (es. user@server:2222)"))?;
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

pub fn run_remote_stats(host: &str) -> Result<RemoteStats, String> {
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
    fn probe_splits_the_port_probes_localhost_and_forces_c_locale() {
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
        assert!(PROBE_SCRIPT.starts_with("LC_ALL=C "));
    }

    #[test]
    fn cpu_is_one_minus_idle() {
        let top = "top - 12:00:00 up 1 day\nTasks: 1 total\n\
                   %Cpu(s):  5.9 us,  1.9 sy,  0.0 ni, 91.9 id,  0.2 wa,  0.0 hi\n";
        assert!((parse_cpu(top).unwrap() - 8.1).abs() < 0.001);
        assert_eq!(parse_cpu("no summary here"), None);
    }

    #[test]
    fn cpu_handles_locale_decimal_commas_without_inverting() {
        // Real `top` output from an Italian locale: splitting on ',' would
        // find "6 id" and report 94% usage on an almost idle machine.
        let top = "top - 11:29:52 up  5:22,  1 user,  load average: 0,96, 1,13, 1,30\n\
                   Tasks: 421 total, 1 running, 420 sleep\n\
                   %Cpu(s):  1,4 us,  1,4 sy,  0,0 ni, 95,6 id,  1,7 wa,  0,0 hi\n";
        let usage = parse_cpu(top).unwrap();
        assert!((usage - 4.4).abs() < 0.001, "got {usage}");
        assert!(usage < 10.0);
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
        // Localised headers must not change the column layout.
        let localized = "File system 1024-blocchi Usati Disponib. Capacità Montato su\n\
                         /dev/sda1 10 4 6 41% /\n";
        assert_eq!(parse_disk(localized).len(), 1);
    }

    #[test]
    fn net_lists_interfaces_without_loopback() {
        let dev = "Inter-|   Receive                                                |  Transmit\n\
                     lo: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0\n\
                   eth0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n\
                   eth1: 500 5 0 0 0 0 0 0 700 7 0 0 0 0 0 0\n";
        assert_eq!(
            parse_net(dev),
            vec![
                NetInterface {
                    name: "eth0".to_string(),
                    rx_bytes: 1000,
                    tx_bytes: 2000,
                },
                NetInterface {
                    name: "eth1".to_string(),
                    rx_bytes: 500,
                    tx_bytes: 700,
                },
            ]
        );
        assert!(parse_net("only a header").is_empty());
    }

    #[test]
    fn net_counters_follow_the_selected_interface() {
        let interfaces = vec![
            NetInterface {
                name: "eth0".to_string(),
                rx_bytes: 1000,
                tx_bytes: 2000,
            },
            NetInterface {
                name: "eth1".to_string(),
                rx_bytes: 500,
                tx_bytes: 700,
            },
        ];
        assert_eq!(net_counters(&interfaces, None), (1500, 2700));
        assert_eq!(net_counters(&interfaces, Some("eth1")), (500, 700));
        // A vanished interface must not reuse another one's counters.
        assert_eq!(net_counters(&interfaces, Some("eth9")), (0, 0));
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
        assert_eq!(stats.net_interfaces.len(), 1);
        assert_eq!(stats.net_interfaces[0].tx_bytes, 20);
        assert_eq!(stats.gpu, None);
    }

    #[test]
    fn connection_choices_put_active_ssh_sessions_first() {
        assert!(same_ssh_host("andres@192.168.1.161:2222", "192.168.1.161"));
        assert!(!same_ssh_host("a@host1", "host2"));
        assert!(!same_ssh_host("", "host"));

        let choices = vec![
            ("first".into(), "first".into(), false),
            ("live".into(), "live".into(), true),
            ("second".into(), "second".into(), false),
            ("other".into(), "other".into(), true),
        ];
        assert_eq!(
            order_connection_choices(choices)
                .into_iter()
                .map(|(target, _, _)| target)
                .collect::<Vec<_>>(),
            vec!["live", "other", "first", "second"]
        );
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
