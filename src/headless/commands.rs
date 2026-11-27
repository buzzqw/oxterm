//! Short external commands that can be interrupted while their output is read.
//!
//! `Command::output()` waits until the child exits and cannot be cancelled:
//! closing the dock used to leave `ssh` (and its `tail`/`find` helpers) running
//! for minutes. We spawn in a new session, drain both pipes on their own
//! threads and poll `try_wait`, so a stop flag can kill the whole process tree
//! promptly. No GTK here.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// Why a cancellable command did not produce output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// The caller set the stop flag; the process group was killed.
    Cancelled,
    /// The program could not be started or waited for.
    Failed(String),
}

/// Stop a child and everything it started. The child leads its own session,
/// so a group kill also reaches helpers that would otherwise keep the pipes
/// open long after the parent is gone.
fn kill_tree(child: &mut Child) {
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn drain<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    })
}

/// Run `command` like `Command::output()`, but return `CommandError::Cancelled`
/// as soon as `cancel` is set, killing the child process group.
pub fn output_cancellable(
    command: &mut Command,
    cancel: &AtomicBool,
) -> Result<Output, CommandError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| {
        if cancel.load(Ordering::SeqCst) {
            CommandError::Cancelled
        } else {
            CommandError::Failed(format!("Impossibile avviare il comando: {error}"))
        }
    })?;
    let mut stdout_reader = child.stdout.take().map(drain);
    let mut stderr_reader = child.stderr.take().map(drain);
    loop {
        if cancel.load(Ordering::SeqCst) {
            kill_tree(&mut child);
            if let Some(handle) = stdout_reader.take() {
                let _ = handle.join();
            }
            if let Some(handle) = stderr_reader.take() {
                let _ = handle.join();
            }
            return Err(CommandError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_reader
                    .take()
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                let stderr = stderr_reader
                    .take()
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                kill_tree(&mut child);
                return Err(CommandError::Failed(format!(
                    "Errore durante l'attesa del comando: {error}"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_command_can_be_cancelled() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let worker_flag = cancel.clone();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            worker_flag.store(true, Ordering::SeqCst);
        });
        let mut command = Command::new("sh");
        command.arg("-c").arg("echo before; sleep 30; echo after");
        let start = std::time::Instant::now();
        let result = output_cancellable(&mut command, &cancel);
        worker.join().unwrap();
        assert_eq!(result, Err(CommandError::Cancelled));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "cancellation must not wait for the 30 s sleep"
        );
    }

    #[test]
    fn a_finished_command_returns_its_output() {
        let cancel = AtomicBool::new(false);
        let mut command = Command::new("sh");
        command.arg("-c").arg("echo ready; echo problem 1>&2");
        let output = output_cancellable(&mut command, &cancel).expect("command ran");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ready");
        assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), "problem");
    }
}
