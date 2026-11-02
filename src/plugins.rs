//! Plugin host: starts installed plugins and forwards events to them.
//!
//! A plugin is an external process. Oxterm gives it:
//!
//! * `OXTERM_CONTROL` — the private control socket;
//! * `OXTERM_PLUGIN_ID`, `OXTERM_PLUGIN_DIR`, `OXTERM_PLUGIN_STATE`;
//! * one JSON event per line on standard input, filtered by the manifest;
//! * a state directory it may use for its own files.
//!
//! The process can react by running `oxterm action ...`, `oxterm new ...` and
//! the other client verbs. Permissions are checked for requests that carry
//! `OXTERM_PLUGIN_ID`; they are a safety rail, not a sandbox: an external
//! process already runs with the user's privileges.
//!
//! A plugin that exits is restarted up to three times, with a growing delay.

use std::collections::HashSet;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::headless::events::{self, EventFilter};
use crate::headless::plugins::{Manifest, Store};
use crate::logging::LOGGER;

static STOP: once_cell::sync::Lazy<Arc<AtomicBool>> =
    once_cell::sync::Lazy::new(|| Arc::new(AtomicBool::new(false)));

/// Plugin ids that currently have a host thread. The manager uses this to show
/// what is running and to avoid starting the same plugin twice.
static RUNNING: once_cell::sync::Lazy<Mutex<HashSet<String>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashSet::new()));

pub fn start_all() {
    start_missing();
}

/// Start every enabled plugin that is not running yet. Safe to call again after
/// installing a plugin from the manager.
pub fn start_missing() {
    if std::env::var_os("OXTERM_NO_PLUGINS").is_some() {
        return;
    }
    let store = Store::default_store();
    let plugins = match store.list() {
        Ok(plugins) => plugins,
        Err(error) => {
            LOGGER.warning(&format!("plugins_list_failed error={}", error));
            return;
        }
    };
    for plugin in plugins {
        if !plugin.manifest.enabled {
            continue;
        }
        let manifest = plugin.manifest;
        let id = manifest.id.clone();
        {
            let Ok(mut running) = RUNNING.lock() else {
                return;
            };
            if running.contains(&id) {
                continue;
            }
            running.insert(id.clone());
        }
        let stop = STOP.clone();
        let store = store.clone();
        let thread_id = id.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("oxterm-plugin-{}", id))
            .spawn(move || {
                run_loop(store, manifest, stop);
                if let Ok(mut running) = RUNNING.lock() {
                    running.remove(&thread_id);
                }
            });
        if let Err(error) = spawned {
            if let Ok(mut running) = RUNNING.lock() {
                running.remove(&id);
            }
            LOGGER.warning(&format!("plugin_thread_failed id={} error={}", id, error));
        }
    }
}

pub fn is_running(id: &str) -> bool {
    RUNNING
        .lock()
        .map(|running| running.contains(id))
        .unwrap_or(false)
}

/// Ask every plugin host thread to stop. The processes themselves also carry a
/// parent-death signal, so they do not outlive a crashed Oxterm.
pub fn stop_all() {
    STOP.store(true, Ordering::SeqCst);
}

fn run_loop(store: Store, manifest: Manifest, stop: Arc<AtomicBool>) {
    let mut attempt = 0u32;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match spawn_once(&store, &manifest) {
            Ok((mut child, stdin)) => {
                LOGGER.info(&format!(
                    "plugin_started id={} attempt={}",
                    manifest.id, attempt
                ));
                let subscription =
                    event_filter(&manifest).and_then(|filter| events::bus().subscribe(filter));
                let forward = match subscription {
                    Some((subscription, receiver)) => {
                        let forward_stop = stop.clone();
                        let forward = std::thread::Builder::new()
                            .name(format!("oxterm-plugin-events-{}", manifest.id))
                            .spawn(move || forward_events(receiver, stdin, forward_stop))
                            .ok();
                        Some((subscription, forward))
                    }
                    None => {
                        // No `events` permission or no subscription left: close
                        // standard input so the plugin sees EOF instead of
                        // waiting forever for a line.
                        drop(stdin);
                        None
                    }
                };
                let status = child.wait();
                if let Some((subscription, forward)) = forward {
                    events::bus().unsubscribe(subscription);
                    if let Some(forward) = forward {
                        let _ = forward.join();
                    }
                }
                LOGGER.info(&format!(
                    "plugin_exited id={} status={:?}",
                    manifest.id, status
                ));
            }
            Err(error) => {
                LOGGER.warning(&format!(
                    "plugin_spawn_failed id={} error={}",
                    manifest.id, error
                ));
            }
        }
        attempt += 1;
        if attempt >= 3 || stop.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_secs(attempt as u64));
    }
}

fn event_filter(manifest: &Manifest) -> Option<EventFilter> {
    if !manifest.has_permission("events") {
        return None;
    }
    Some(EventFilter {
        pane: None,
        events: if manifest.events.is_empty() {
            None
        } else {
            Some(manifest.events.clone())
        },
        include_output: manifest.has_permission("read-screen")
            && (manifest.events.is_empty()
                || manifest.events.iter().any(|name| name == "output_line")),
    })
}

fn spawn_once(
    store: &Store,
    manifest: &Manifest,
) -> Result<(Child, std::process::ChildStdin), String> {
    use std::os::unix::process::CommandExt;
    let dir = store.plugin_dir(&manifest.id);
    let state = store.state_dir(&manifest.id);
    std::fs::create_dir_all(&state)
        .map_err(|error| format!("cannot create {}: {}", state.display(), error))?;
    std::fs::set_permissions(&state, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .map_err(|error| format!("cannot secure {}: {}", state.display(), error))?;
    let log_path = state.join("plugin.log");
    if std::fs::metadata(&log_path)
        .map(|metadata| metadata.len() > 1024 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::write(&log_path, b"");
    }
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| format!("cannot open {}: {}", log_path.display(), error))?;
    let _ = writeln!(log, "--- {} {} starting ---", manifest.id, manifest.version);
    let log_err = log
        .try_clone()
        .map_err(|error| format!("cannot clone the plugin log: {}", error))?;

    let mut argv = manifest.command.clone();
    if !argv[0].contains('/') && dir.join(&argv[0]).is_file() {
        argv[0] = format!("./{}", argv[0]);
    }
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(&dir)
        .env("OXTERM_CONTROL", crate::control::socket_path())
        .env("OXTERM_PLUGIN_ID", &manifest.id)
        .env("OXTERM_PLUGIN_DIR", &dir)
        .env("OXTERM_PLUGIN_STATE", &state)
        .env("OXTERM_PLUGIN_PERMISSIONS", manifest.permissions.join(","))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_err));
    if let Ok(executable) = std::env::current_exe() {
        command.env("OXTERM", executable);
    }
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start {}: {}", argv[0], error))?;
    let stdin = child
        .stdin
        .take()
        .ok_or("the plugin has no standard input")?;
    Ok((child, stdin))
}

fn forward_events(
    receiver: std::sync::mpsc::Receiver<Value>,
    mut stdin: std::process::ChildStdin,
    stop: Arc<AtomicBool>,
) {
    for event in receiver {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let line = event.to_string();
        if writeln!(stdin, "{}", line).is_err() {
            break;
        }
        if stdin.flush().is_err() {
            break;
        }
    }
}
