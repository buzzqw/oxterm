use std::fs;

fn mb(val: u64) -> String {
    let gb = 1024 * 1024 * 1024;
    let mb = 1024 * 1024;
    if val >= gb {
        format!("{:.1}G", val as f64 / gb as f64)
    } else {
        format!("{}M", val / mb)
    }
}

fn parse_kmg_swap(unit: &str, factor: u64) -> u64 {
    unit.trim().parse::<f64>().unwrap_or(0.0) as u64 * factor
}

fn meminfo() -> (u64, u64) {
    let Ok(content) = fs::read_to_string("/proc/meminfo") else {
        return (0, 0);
    };
    let mut total = 0u64;
    let mut avail = 0u64;
    for line in content.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = parse_kmg_swap(v, 1024);
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            avail = parse_kmg_swap(v, 1024);
        }
    }
    let used = total.saturating_sub(avail);
    (used, total)
}

fn stat_cpu() -> Option<(u64, u64)> {
    let content = fs::read_to_string("/proc/stat").ok()?;
    let line = content.lines().next()?;
    let mut parts = line.split_whitespace();
    let _ = parts.next();
    let vals: Vec<u64> = parts.filter_map(|p| p.parse().ok()).collect();
    let total: u64 = vals.iter().sum();
    let idle: u64 = vals.get(3).copied().unwrap_or(0) + vals.get(4).copied().unwrap_or(0);
    Some((idle, total))
}

fn cpu_percent(prev: &mut Option<(u64, u64)>) -> f64 {
    match stat_cpu() {
        Some(cur) => {
            if let Some((p_idle, p_total)) = prev {
                let d_total = cur.1.saturating_sub(*p_total);
                let d_idle = cur.0.saturating_sub(*p_idle);
                if d_total > 0 {
                    let pct = (d_total - d_idle) as f64 / d_total as f64 * 100.0;
                    *prev = Some(cur);
                    return pct;
                }
            }
            *prev = Some(cur);
            0.0
        }
        None => 0.0,
    }
}

fn disk_usage(path: &str) -> (u64, u64) {
    let Ok(meta) = fs::metadata(path) else {
        return (0, 0);
    };
    let Ok(stats) = nix::sys::statvfs::statvfs(path) else {
        return (0, 0);
    };
    let _ = meta;
    let block_size = stats.fragment_size();
    let total = stats.blocks() as u64 * block_size;
    let free = stats.blocks_free() as u64 * block_size;
    let avail = stats.blocks_available() as u64 * block_size;
    let used = total.saturating_sub(free.max(avail));
    (used, total)
}

static CPU_PREV: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);

pub fn collect(is_ssh: bool) -> String {
    if is_ssh {
        return ssh_placeholder();
    }
    let mut prev = CPU_PREV.lock().unwrap();
    let cpu = cpu_percent(&mut prev);
    drop(prev);
    let (mem_used, mem_total) = meminfo();
    let (disk_used, disk_total) = disk_usage("/");
    let mem_pct = if mem_total > 0 {
        (mem_used as f64 / mem_total as f64 * 100.0) as i64
    } else {
        0
    };
    let disk_pct = if disk_total > 0 {
        (disk_used as f64 / disk_total as f64 * 100.0) as i64
    } else {
        0
    };
    format!(
        "  CPU {:5.1}%  RAM {}/{} ({}%)  Disk {}/{} ({}%)",
        cpu,
        mb(mem_used),
        mb(mem_total),
        mem_pct,
        mb(disk_used),
        mb(disk_total),
        disk_pct
    )
}

pub fn collect_self() -> String {
    let pid = std::process::id();
    let rss = proc_rss(pid);
    let cpu = process_cpu_percent(pid);
    format!("TPGK  CPU {:5.1}%  RAM {}  ", cpu, mb(rss))
}

fn proc_rss(pid: u32) -> u64 {
    let Ok(content) = fs::read_to_string(format!("/proc/{}/statm", pid)) else {
        return 0;
    };
    let rss_pages = content
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    rss_pages * page_size
}

fn process_cpu_percent(pid: u32) -> f64 {
    let Ok(content) = fs::read_to_string(format!("/proc/{}/stat", pid)) else {
        return 0.0;
    };
    // /proc/pid/stat: field 3 is state, fields 14 and 15 are utime/stime.
    let after_comm = content.split(')').nth(1).unwrap_or("");
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime: u64 = fields.get(11).and_then(|v| v.parse().ok()).unwrap_or(0);
    let stime: u64 = fields.get(12).and_then(|v| v.parse().ok()).unwrap_or(0);
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    if hz <= 0.0 {
        return 0.0;
    }
    (utime + stime) as f64 / hz * 100.0
}

pub fn ssh_placeholder() -> String {
    "  [SSH] Remote session".to_string()
}
