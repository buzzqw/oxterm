//! Tests for the headless core.
//!
//! These run without a display and without the GUI modules, which is the point
//! of `src/headless`: parsing, settings, framing and VI motions can be verified
//! on any machine, including CI containers without X11.

use std::fs;
use std::path::Path;

use oxterm::headless::{cli, control_cli, framing, settings, spec, text, vi};

#[test]
fn headless_sources_do_not_reach_into_the_gui() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/headless");
    let mut checked = 0;
    for entry in fs::read_dir(&directory).expect("read headless directory") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        checked += 1;
        let source = fs::read_to_string(&path).unwrap();
        for (number, line) in source.lines().enumerate() {
            let code = line.trim();
            if code.starts_with("//") || code.starts_with("///") || code.starts_with("//!") {
                continue;
            }
            for forbidden in [
                "gtk::",
                "gdk::",
                "glib::",
                "zoha_vte",
                "crate::terminal",
                "crate::window",
                "crate::settings",
                "crate::control",
            ] {
                assert!(
                    !code.contains(forbidden),
                    "{}:{} uses {forbidden}; the headless core must not depend on the GUI",
                    path.display(),
                    number + 1
                );
            }
        }
    }
    assert!(checked >= 7, "expected the headless modules to be scanned");
}

#[test]
fn cli_help_and_inspection_need_no_display() {
    let help = cli::parse_cli(&["--help".to_string()]).unwrap();
    assert!(help.help);
    assert!(cli::usage().contains("--show-config"));
    assert!(cli::usage().contains("split [--pane"));

    let themes = cli::parse_cli(&["--list-themes".to_string()]).unwrap();
    assert_eq!(themes.inspection(), Some(cli::Inspection::ListThemes));
    assert!(
        themes.start_dir.is_none(),
        "inspection must not resolve a directory"
    );
}

#[test]
fn settings_spec_round_trips_through_a_document() {
    let document = r#"{"font_size": 18, "unknown_key": true}"#;
    let loaded = settings::load_document(document).unwrap();
    assert_eq!(loaded.settings["font_size"], serde_json::json!(18));
    assert!(loaded.issues.iter().any(|issue| issue.key == "unknown_key"));
    assert!(
        !loaded
            .issues
            .iter()
            .find(|i| i.key == "unknown_key")
            .unwrap()
            .fatal
    );
    for setting in spec::SETTINGS {
        assert!(
            spec::validate(setting, &setting.default_value()).is_ok(),
            "default of {} is not valid",
            setting.key
        );
    }
}

#[test]
fn control_protocol_parsing_is_independent_of_the_socket() {
    let args: Vec<String> = ["oxterm", "send", "--pane", "42", "--enter", "make", "test"]
        .iter()
        .map(|value| value.to_string())
        .collect();
    let mode = control_cli::parse_cli_mode(&args).unwrap().unwrap();
    assert_eq!(
        mode,
        control_cli::CliMode::Send {
            pane: Some("42".to_string()),
            enter: true,
            text: Some("make test".to_string()),
        }
    );
}

#[test]
fn framing_and_vi_helpers_are_exercised_without_a_gui() {
    let mut buffer = Vec::new();
    framing::write_json(&mut buffer, &serde_json::json!({"ok": true}), 1024).unwrap();
    let value = framing::read_json(&mut buffer.as_slice(), 1024).unwrap();
    assert_eq!(value["ok"], true);
    assert!(framing::read_json(&mut &b"\xff\xff\xff\xff"[..], 1024).is_err());

    assert_eq!(vi::next_word_start("one two", 0, false), Some(4));
    assert_eq!(vi::chars_slice("città", 0, 4), "città");
    assert_eq!(text::truncate_chars("città", 4), "citt");
}
