use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn main() {
    // Re-run after commits on the current branch so the displayed build number
    // is refreshed even when no Rust source file changed.
    if let Some(head_path) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }
    if let Some(branch) = git(&["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        if let Some(ref_path) = git(&["rev-parse", "--git-path", &format!("refs/heads/{branch}")]) {
            println!("cargo:rerun-if-changed={ref_path}");
        }
    }

    let commit_count = git(&["rev-list", "--count", "HEAD"]).unwrap_or_else(|| "0".into());
    // Tarball builds have no Git metadata, so no suffix is added there; Git
    // builds expose the commit count as SemVer build metadata (1.1.11+116).
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let full_version = if commit_count.trim() == "0" {
        version
    } else {
        format!("{version}+{}", commit_count.trim())
    };
    println!("cargo:rustc-env=OXTERM_VERSION={full_version}");
    println!("cargo:rustc-env=OXTERM_COMMIT_COUNT={commit_count}");
}
