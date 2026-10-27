//! Regenerate the committed documentation from the headless spec.
//!
//! `cargo run --bin oxterm-docgen` rewrites the generated blocks in the man
//! page and the shell completions, plus `docs/settings.md`.
//! `--check` verifies that the committed files are current and is what CI runs.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use oxterm::headless::spec;

fn main() -> ExitCode {
    let check = std::env::args().skip(1).any(|arg| arg == "--check");
    match run(check) {
        Ok(changed) => {
            if check && !changed.is_empty() {
                eprintln!("Generated documentation is out of date:");
                for file in changed {
                    eprintln!("  {file}");
                }
                eprintln!("Run `cargo run --bin oxterm-docgen` and commit the result.");
                ExitCode::FAILURE
            } else {
                for file in changed {
                    println!("updated {file}");
                }
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("oxterm-docgen: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(check: bool) -> Result<Vec<&'static str>, String> {
    let mut changed = Vec::new();
    let mut update =
        |path: &'static str, begin: &str, end: &str, body: String| -> Result<(), String> {
            if replace_block(Path::new(path), begin, end, &body, check)? {
                changed.push(path);
            }
            Ok(())
        };

    update(
        "packaging/oxterm.1",
        ".\\\" BEGIN GENERATED OPTIONS",
        ".\\\" END GENERATED OPTIONS",
        spec::man_options(),
    )?;
    update(
        "packaging/completions/oxterm.bash",
        "# BEGIN GENERATED OPTIONS",
        "# END GENERATED OPTIONS",
        format!("    {}\n", spec::bash_option_words()),
    )?;
    update(
        "packaging/completions/oxterm.bash",
        "# BEGIN GENERATED CASES",
        "# END GENERATED CASES",
        spec::bash_value_cases(),
    )?;
    update(
        "packaging/completions/oxterm.fish",
        "# BEGIN GENERATED OPTIONS",
        "# END GENERATED OPTIONS",
        spec::fish_options(),
    )?;
    update(
        "packaging/completions/_oxterm",
        "# BEGIN GENERATED OPTIONS",
        "# END GENERATED OPTIONS",
        spec::zsh_options(),
    )?;

    update(
        "README.md",
        "<!-- BEGIN GENERATED HELP -->",
        "<!-- END GENERATED HELP -->",
        format!("```text\n{}```\n", spec::usage()),
    )?;
    update(
        "manual.md",
        "<!-- BEGIN GENERATED KEYBINDS -->",
        "<!-- END GENERATED KEYBINDS -->",
        spec::keybinds_markdown(),
    )?;

    let settings = "docs/settings.md";
    let body = spec::settings_markdown();
    if replace_full(Path::new(settings), &body, check)? {
        changed.push(settings);
    }
    Ok(changed)
}

/// Replace the text between two marker lines, keeping the markers.
fn replace_block(
    path: &Path,
    begin: &str,
    end: &str,
    body: &str,
    check: bool,
) -> Result<bool, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let start = text
        .find(begin)
        .ok_or_else(|| format!("{}: missing marker {begin:?}", path.display()))?;
    let body_start = start + begin.len();
    let end_offset = text[body_start..]
        .find(end)
        .ok_or_else(|| format!("{}: missing marker {end:?}", path.display()))?;
    let end_at = body_start + end_offset;
    let mut body = body.to_string();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    // Keep the indentation of the closing marker: the body itself carries the
    // indentation of its first line.
    let end_line_start = text[..end_at]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let updated = format!(
        "{}\n{}{}",
        &text[..body_start],
        body,
        &text[end_line_start..]
    );
    if updated == text {
        return Ok(false);
    }
    if !check {
        fs::write(path, &updated).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(true)
}

/// Replace a whole generated file.
fn replace_full(path: &Path, body: &str, check: bool) -> Result<bool, String> {
    let mut body = body.to_string();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    let current = fs::read_to_string(path).unwrap_or_default();
    if current == body {
        return Ok(false);
    }
    if !check {
        fs::write(path, &body).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(true)
}
