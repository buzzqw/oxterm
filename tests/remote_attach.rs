use std::io::{self, Read, Write};
use std::os::fd::RawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const FRAME_INPUT: u8 = 1;
const FRAME_DETACH: u8 = 2;

fn write_frame(stream: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(payload)
}

fn read_frame(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let mut payload = vec![0u8; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

fn open_pty() -> io::Result<(RawFd, RawFd)> {
    let mut master = -1;
    let mut slave = -1;
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        Ok((master, slave))
    } else {
        Err(io::Error::last_os_error())
    }
}

fn write_fd(fd: RawFd, data: &[u8]) -> io::Result<()> {
    let count = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
    if count == data.len() as isize {
        Ok(())
    } else if count < 0 {
        Err(io::Error::last_os_error())
    } else {
        Err(io::Error::new(io::ErrorKind::WriteZero, "short PTY write"))
    }
}

fn binary_path() -> PathBuf {
    let variable = format!("CARGO_BIN_EXE_{}", env!("CARGO_PKG_NAME"));
    if let Some(path) = std::env::var_os(variable) {
        return path.into();
    }
    std::env::current_exe()
        .expect("test executable path")
        .parent()
        .and_then(|path| path.parent())
        .expect("target directory")
        .join(env!("CARGO_PKG_NAME"))
}

fn wait_for_exit(child: &mut Child) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "attach relay did not exit",
    ))
}

#[test]
fn attach_forwards_input_and_detaches_cleanly() -> io::Result<()> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let home = dirs::home_dir().expect("home directory");
    let remote_dir = home.join(".config/tpgk/remote");
    std::fs::create_dir_all(&remote_dir)?;
    let session_id = format!("attach-test-{}-{}", std::process::id(), unique);
    let socket = remote_dir.join(format!("trust-{}.sock", session_id));
    let listener = UnixListener::bind(&socket)?;
    let (attached_tx, attached_rx) = mpsc::channel();
    let server_session_id = session_id.clone();
    let server = thread::spawn(move || -> io::Result<Vec<u8>> {
        let (mut stream, attach) = loop {
            let (mut stream, _) = listener.accept()?;
            match read_frame(&mut stream) {
                Ok(attach) => break (stream, attach),
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => continue,
                Err(error) => return Err(error),
            }
        };
        assert_eq!(attach, format!("ATTACH {}", server_session_id).as_bytes());
        write_frame(&mut stream, b"OK\nATTACH")?;
        attached_tx.send(()).unwrap();
        let mut input = Vec::new();
        loop {
            let frame = read_frame(&mut stream)?;
            if frame.first() == Some(&FRAME_INPUT) {
                input.extend_from_slice(&frame[1..]);
            } else if frame.first() == Some(&FRAME_DETACH) {
                return Ok(input);
            }
        }
    });

    let (master, slave) = open_pty()?;
    let slave_for_child = slave;
    let mut child = unsafe {
        let mut command = Command::new(binary_path());
        command
            .args(["--attach", &session_id])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .pre_exec(move || {
                if libc::dup2(slave_for_child, 0) < 0
                    || libc::dup2(slave_for_child, 1) < 0
                    || libc::dup2(slave_for_child, 2) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                libc::close(slave_for_child);
                Ok(())
            })
            .spawn()?
    };
    unsafe { libc::close(slave) };

    attached_rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "attach handshake timed out"))?;
    write_fd(master, b"x")?;
    write_fd(master, &[0x02, b'd'])?;
    let input = server
        .join()
        .map_err(|_| io::Error::other("attach server panicked"))??;
    wait_for_exit(&mut child)?;
    assert_eq!(input, b"x");

    unsafe { libc::close(master) };
    let _ = std::fs::remove_file(socket);
    Ok(())
}
