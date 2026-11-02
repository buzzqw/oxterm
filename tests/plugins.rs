//! `oxterm plugin` is a headless command: these tests do not need a display.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_oxterm")
}

fn temp_root(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("oxterm-{}-{}-{}", name, std::process::id(), unique));
    std::fs::create_dir_all(&path).expect("create temp root");
    path
}

fn run(home: &Path, data: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env("HOME", home)
        .env("XDG_DATA_HOME", data)
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .output()
        .expect("run oxterm")
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn plugin_cli_installs_lists_and_removes() {
    let root = temp_root("plugin-cli");
    let home = root.join("home");
    let data = root.join("data");
    let source = root.join("source");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(
        source.join("plugin.json"),
        r#"{
            "id": "acme.test",
            "name": "Test plugin",
            "version": "1.2.3",
            "api_version": 1,
            "permissions": ["notify", "events"],
            "events": ["command_finished"],
            "command": ["./run.sh"]
        }"#,
    )
    .expect("manifest");
    std::fs::write(source.join("run.sh"), "#!/bin/sh\nexit 0\n").expect("script");

    let installed = stdout(&run(
        &home,
        &data,
        &["plugin", "install", source.to_str().unwrap()],
    ));
    assert!(
        installed.contains("installed acme.test 1.2.3"),
        "{installed}"
    );
    assert!(installed.contains("notify, events"), "{installed}");

    let list = stdout(&run(&home, &data, &["plugin", "list"]));
    assert!(list.contains("acme.test"), "{list}");
    assert!(list.contains("1.2.3"), "{list}");
    assert!(list.contains("notify,events"), "{list}");

    let permissions = stdout(&run(&home, &data, &["plugin", "permissions", "acme.test"]));
    assert!(permissions.contains("notify, events"), "{permissions}");

    let removed = stdout(&run(
        &home,
        &data,
        &["plugin", "remove", "acme.test", "--purge"],
    ));
    assert!(removed.contains("removed acme.test"), "{removed}");
    let list = stdout(&run(&home, &data, &["plugin", "list"]));
    assert!(list.contains("No plugins installed"), "{list}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn plugin_install_rejects_a_bad_manifest() {
    let root = temp_root("plugin-bad");
    let home = root.join("home");
    let data = root.join("data");
    let source = root.join("source");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(
        source.join("plugin.json"),
        r#"{"id":"bad/id","version":"1","api_version":1,"command":["x"]}"#,
    )
    .expect("manifest");
    let output = run(
        &home,
        &data,
        &["plugin", "install", source.to_str().unwrap()],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("plugin id"));
    let _ = std::fs::remove_dir_all(&root);
}
