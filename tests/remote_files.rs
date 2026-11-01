//! The file browser's engine against the real `sftp` and a private `sshd`.
//!
//! Set `OXTERM_E2E=1` to run it: it starts an unprivileged server on a free port.
//! `sshd` needs an absolute path and host keys, which not every machine has.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use oxterm::headless::connections::{Connection, Protocol};
use oxterm::headless::remote_files as files;

struct Server {
    child: Child,
    dir: PathBuf,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn start_server() -> Option<Server> {
    if std::env::var_os("OXTERM_E2E").is_none() {
        eprintln!("skipping: set OXTERM_E2E=1");
        return None;
    }
    let sshd = ["/usr/bin/sshd", "/usr/sbin/sshd"]
        .into_iter()
        .find(|path| Path::new(path).exists())?;
    let sftp_server = [
        "/usr/lib/ssh/sftp-server",
        "/usr/lib/openssh/sftp-server",
        "/usr/libexec/openssh/sftp-server",
    ]
    .into_iter()
    .find(|path| Path::new(path).exists())?;
    let dir = std::env::temp_dir().join(format!("oxrf{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).ok()?;
    let keygen = |name: &str| {
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(dir.join(name))
            .status()
            .is_ok_and(|status| status.success())
    };
    if !keygen("host") || !keygen("user") {
        return None;
    }
    fs::copy(dir.join("user.pub"), dir.join("authorized_keys")).ok()?;
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()?
        .local_addr()
        .ok()?
        .port();
    fs::write(
        dir.join("sshd_config"),
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host\nAuthorizedKeysFile {d}/authorized_keys\n\
             PidFile {d}/pid\nStrictModes no\nUsePAM no\nPasswordAuthentication no\n\
             Subsystem sftp {sftp_server}\n",
            d = dir.display()
        ),
    )
    .ok()?;
    let child = Command::new(sshd)
        .args(["-D", "-e", "-f"])
        .arg(dir.join("sshd_config"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    fs::write(
        dir.join("ssh_config"),
        format!(
            "Host *\n  UserKnownHostsFile {d}/known\n  StrictHostKeyChecking accept-new\n  \
             IdentityFile {d}/user\n  IdentitiesOnly yes\n  ControlPath none\n",
            d = dir.display()
        ),
    )
    .ok()?;
    Some(Server { child, dir, port })
}

fn connection(server: &Server) -> Connection {
    Connection {
        id: "t".into(),
        name: "t".into(),
        protocol: Protocol::Sftp,
        host: "127.0.0.1".into(),
        port: Some(server.port),
        extra_args: vec![
            "-F".into(),
            server.dir.join("ssh_config").display().to_string(),
        ],
        ..Connection::default()
    }
}

#[test]
fn browsing_copying_renaming_and_deleting_work_on_a_real_server() {
    let Some(server) = start_server() else {
        return;
    };
    let connection = connection(&server);
    let area = server.dir.join("area");
    let local = server.dir.join("local");
    fs::create_dir_all(&area).unwrap();
    fs::create_dir_all(local.join("folder/inner")).unwrap();
    fs::write(local.join("plain.txt"), "hello").unwrap();
    fs::write(local.join("two  spaces \"q\".txt"), "odd").unwrap();
    fs::write(local.join("folder/inner/deep.txt"), "deep").unwrap();
    let area_text = area.display().to_string();
    let local_text = local.display().to_string();

    assert!(!files::home_directory(&connection).unwrap().is_empty());

    // Upload a file, an awkward name, and a folder.
    for (name, is_dir) in [
        ("plain.txt", false),
        ("two  spaces \"q\".txt", false),
        ("folder", true),
    ] {
        files::transfer(
            &connection,
            true,
            is_dir,
            &files::join(&local_text, name),
            &files::join(&area_text, name),
        )
        .unwrap();
    }
    assert_eq!(
        fs::read_to_string(area.join("folder/inner/deep.txt")).unwrap(),
        "deep"
    );
    assert_eq!(
        fs::read_to_string(area.join("two  spaces \"q\".txt")).unwrap(),
        "odd"
    );

    let entries = files::list(&connection, &area_text).unwrap();
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["folder", "plain.txt", "two  spaces \"q\".txt"]);
    assert!(entries[0].is_dir);
    assert_eq!(entries[1].size, 5);

    // Download, make a folder, rename.
    let back = local.join("back.txt");
    files::transfer(
        &connection,
        false,
        false,
        &files::join(&area_text, "plain.txt"),
        &back.display().to_string(),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&back).unwrap(), "hello");
    let protocol = connection.protocol;
    files::run(
        &connection,
        &files::script(
            protocol,
            &files::Operation::MakeDir(files::join(&area_text, "made")),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(area.join("made").is_dir());
    files::run(
        &connection,
        &files::script(
            protocol,
            &files::Operation::Rename {
                from: files::join(&area_text, "plain.txt"),
                to: files::join(&area_text, "renamed.txt"),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(area.join("renamed.txt").exists());

    // A folder goes with what it holds; a link goes as a link.
    let outside = server.dir.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "keep").unwrap();
    std::os::unix::fs::symlink(&outside, area.join("folder/link")).unwrap();
    files::delete(&connection, &files::join(&area_text, "folder"), true).unwrap();
    assert!(!area.join("folder").exists());
    assert!(
        outside.join("keep.txt").exists(),
        "deleting must not follow a link"
    );

    // Errors come back as text instead of an empty listing.
    assert!(files::list(&connection, "/nonexistent/oxterm").is_err());
}

#[test]
fn ftp_works_against_a_real_server() {
    if std::env::var_os("OXTERM_E2E").is_none() {
        return;
    }
    let lftp = Command::new("lftp").arg("--version").output().is_ok();
    let library = Command::new("python3")
        .args(["-I", "-c", "import pyftpdlib"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !lftp || !library {
        eprintln!("skipping: needs lftp and python-pyftpdlib");
        return;
    }
    let dir = std::env::temp_dir().join(format!("oxrp{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let root = dir.join("root");
    let local = dir.join("local");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(local.join("up/inner")).unwrap();
    fs::write(root.join("there.txt"), "there").unwrap();
    fs::write(local.join("here.txt"), "here").unwrap();
    fs::write(local.join("up/inner/deep.txt"), "deep").unwrap();
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let script = format!(
        "from pyftpdlib.authorizers import DummyAuthorizer\n\
         from pyftpdlib.handlers import FTPHandler\n\
         from pyftpdlib.servers import FTPServer\n\
         a=DummyAuthorizer();a.add_user('tester','se,cret\\\"x','{}',perm='elradfmwMT')\n\
         class H(FTPHandler):authorizer=a\n\
         FTPServer(('127.0.0.1',{}),H).serve_forever()\n",
        root.display(),
        port
    );
    let mut server = Command::new("python3")
        .args(["-I", "-c", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "the FTP server did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    // A fake keyring: this password has the comma and quote that break naive quoting.
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        bin.join("secret-tool"),
        "#!/bin/sh\nprintf '%s\\n' 'se,cret\"x'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(bin.join("secret-tool"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    std::env::set_var("PATH", path);

    let connection = Connection {
        id: "f".into(),
        name: "f".into(),
        protocol: Protocol::Ftp,
        host: "127.0.0.1".into(),
        port: Some(port),
        user: "tester".into(),
        auth: oxterm::headless::connections::Auth::SecretService,
        ..Connection::default()
    };
    let outcome = std::panic::catch_unwind(|| {
        let entries = files::list(&connection, "/").unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["sub", "there.txt"]);
        assert!(entries[0].is_dir);
        let back = dir.join("local/there.txt");
        files::transfer(
            &connection,
            false,
            false,
            "/there.txt",
            &back.display().to_string(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&back).unwrap(), "there");
        files::transfer(
            &connection,
            true,
            false,
            &dir.join("local/here.txt").display().to_string(),
            "/here.txt",
        )
        .unwrap();
        assert_eq!(fs::read_to_string(root.join("here.txt")).unwrap(), "here");
        files::transfer(
            &connection,
            true,
            true,
            &dir.join("local/up").display().to_string(),
            "/up",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("up/inner/deep.txt")).unwrap(),
            "deep"
        );
        let down = dir.join("local/down");
        files::transfer(&connection, false, true, "/up", &down.display().to_string()).unwrap();
        assert_eq!(
            fs::read_to_string(down.join("inner/deep.txt")).unwrap(),
            "deep"
        );
        files::delete(&connection, "/up", true).unwrap();
        assert!(!root.join("up").exists());
        files::delete(&connection, "/here.txt", false).unwrap();
        assert!(!root.join("here.txt").exists());
    });
    let _ = server.kill();
    let _ = server.wait();
    let _ = fs::remove_dir_all(&dir);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
