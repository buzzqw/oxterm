use std::io::{self, Read, Write};
use std::os::fd::RawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const FRAME_OUTPUT: u8 = 0;
const FRAME_INPUT: u8 = 1;
const FRAME_DETACH: u8 = 2;
const MAX_FRAME: usize = 1024 * 1024;

fn write_frame(stream: &mut UnixStream, payload: &[u8]) -> io::Result<()> {
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(payload)
}

fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    assert!(length <= MAX_FRAME);
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

fn control_request(path: &PathBuf, command: &str) -> io::Result<Vec<u8>> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    write_frame(&mut stream, command.as_bytes())?;
    read_frame(&mut stream)
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

fn set_raw(fd: RawFd) -> io::Result<()> {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut termios = unsafe { termios.assume_init() };
    unsafe { libc::cfmakeraw(&mut termios) };
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn write_fd(fd: RawFd, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let count = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        data = &data[count as usize..];
    }
    Ok(())
}

fn read_fd(fd: RawFd, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let result = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
    if result == 0 {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "PTY read timed out",
        ));
    }
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut data = vec![0u8; 4096];
    let count = unsafe { libc::read(fd, data.as_mut_ptr().cast(), data.len()) };
    if count < 0 {
        Err(io::Error::last_os_error())
    } else {
        data.truncate(count as usize);
        Ok(data)
    }
}

fn no_fd_data(fd: RawFd, timeout: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    unsafe { libc::poll(&mut poll, 1, timeout_ms) == 0 }
}

fn wait_for_socket(path: &PathBuf, child: &mut Child) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!("broker exited early: {status}")));
        }
        if UnixStream::connect(path).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "broker socket did not appear",
    ))
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
        "broker did not exit",
    ))
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

struct Fixture {
    child: Child,
    child_slave: RawFd,
    gui_master: RawFd,
    socket: PathBuf,
    directory: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        unsafe {
            libc::close(self.child_slave);
            libc::close(self.gui_master);
        }
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[test]
fn broker_round_trips_control_and_terminal_traffic() -> io::Result<()> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "trust-broker-test-{}-{}.sock",
        std::process::id(),
        unique
    ));
    std::fs::create_dir(&directory)?;
    let socket = directory.join("session.sock");
    let (child_master, child_slave) = open_pty()?;
    let (gui_master, gui_slave) = open_pty()?;
    set_raw(child_slave)?;
    set_raw(gui_slave)?;

    let broker_socket = socket.clone();
    let child = unsafe {
        let mut command = Command::new(binary_path());
        command
            .arg("--broker")
            .arg(&broker_socket)
            .arg("test-session")
            .env("TRUST_BROKER_TITLE", "Integration test")
            .env("TRUST_BROKER_CWD", "/tmp")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(move || {
                if libc::dup2(child_master, 3) < 0 || libc::dup2(gui_slave, 4) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
            .spawn()?
    };
    unsafe {
        libc::close(child_master);
        libc::close(gui_slave);
    }
    let mut fixture = Fixture {
        child,
        child_slave,
        gui_master,
        socket,
        directory,
    };
    wait_for_socket(&fixture.socket, &mut fixture.child)?;

    let mode = std::fs::metadata(&fixture.socket)?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let listed = control_request(&fixture.socket, "LIST")?;
    let listed = String::from_utf8_lossy(&listed);
    assert!(listed.starts_with("OK\ntest-session\t\tIntegration test\t/tmp\tlocal"));

    assert_eq!(
        control_request(
            &fixture.socket,
            "UPDATE\ttest-session\tbuild\tBuild\t/var/tmp"
        )?,
        b"OK"
    );
    let info = control_request(&fixture.socket, "INFO test-session")?;
    let info = String::from_utf8_lossy(&info);
    assert!(info.contains("test-session\tbuild\tBuild\t/var/tmp\tlocal"));
    assert_eq!(
        control_request(&fixture.socket, "RENAME test-session renamed")?,
        b"OK"
    );
    let info = control_request(&fixture.socket, "INFO test-session")?;
    assert!(String::from_utf8_lossy(&info).contains("test-session\trenamed\t"));

    let mut first = UnixStream::connect(&fixture.socket)?;
    first.set_read_timeout(Some(Duration::from_secs(2)))?;
    write_frame(&mut first, b"ATTACH test-session")?;
    assert_eq!(read_frame(&mut first)?, b"OK\nATTACH");
    let mut second = UnixStream::connect(&fixture.socket)?;
    second.set_read_timeout(Some(Duration::from_secs(2)))?;
    write_frame(&mut second, b"ATTACH test-session")?;
    assert_eq!(read_frame(&mut second)?, b"OK\nATTACH");

    write_frame(&mut first, &[FRAME_INPUT, b'p', b'i', b'n', b'g'])?;
    assert_eq!(
        read_fd(fixture.child_slave, Duration::from_secs(2))?,
        b"ping"
    );

    write_fd(fixture.child_slave, b"broadcast")?;
    let output = read_frame(&mut first)?;
    assert_eq!(output.first(), Some(&FRAME_OUTPUT));
    assert_eq!(&output[1..], b"broadcast");
    let output = read_frame(&mut second)?;
    assert_eq!(output.first(), Some(&FRAME_OUTPUT));
    assert_eq!(&output[1..], b"broadcast");
    assert_eq!(
        read_fd(fixture.gui_master, Duration::from_secs(2))?,
        b"broadcast"
    );

    assert_eq!(
        control_request(&fixture.socket, "LOCAL_OFF test-session")?,
        b"OK"
    );
    write_fd(fixture.child_slave, b"remote-only")?;
    let output = read_frame(&mut first)?;
    assert_eq!(&output[1..], b"remote-only");
    let output = read_frame(&mut second)?;
    assert_eq!(&output[1..], b"remote-only");
    assert!(no_fd_data(fixture.gui_master, Duration::from_millis(100)));

    assert_eq!(
        control_request(&fixture.socket, "LOCAL_ON test-session")?,
        b"OK"
    );
    write_fd(fixture.child_slave, b"local-again")?;
    let output = read_frame(&mut first)?;
    assert_eq!(&output[1..], b"local-again");
    assert_eq!(
        read_fd(fixture.gui_master, Duration::from_secs(2))?,
        b"local-again"
    );

    write_frame(&mut first, &[FRAME_DETACH])?;
    assert_eq!(read_frame(&mut first)?, b"OK\nDETACH");
    assert_eq!(
        control_request(&fixture.socket, "DETACH test-session")?,
        b"OK\nDETACH"
    );
    assert_eq!(
        control_request(&fixture.socket, "KILL test-session")?,
        b"OK"
    );
    wait_for_exit(&mut fixture.child)?;
    assert!(!fixture.socket.exists());
    Ok(())
}
