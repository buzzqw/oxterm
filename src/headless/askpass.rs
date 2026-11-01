//! Decisions of `oxterm-askpass`, and the keyring it reads.
//!
//! With `SSH_ASKPASS_REQUIRE=force`, `ssh` sends *every* prompt to the helper:
//! the password, but also the host-key confirmation, key passphrases and
//! one-time codes. Answering the wrong one is a security bug, so the rule is an
//! allow-list of the password prompt that names the user and host this
//! connection targets. A `ProxyJump` bastion asks with its own name and must
//! never receive the target's password; a generic `Password:` could come from
//! either, so it is not answered. Everything else goes to the terminal, where
//! the person decides.
//!
//! Secrets stay out of argument vectors and the environment: the helper
//! receives the connection id and the expected prompt, and the keyring is read
//! through `secret-tool`, with the password crossing stdin and stdout only.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::paths;

pub const ENV_ID: &str = "OXTERM_ASKPASS_ID";
pub const ENV_TARGET: &str = "OXTERM_ASKPASS_TARGET";
pub const HELPER_NAME: &str = "oxterm-askpass";

const SECRET_ATTRIBUTE: &str = "oxterm-connection";
/// A marker this old belongs to an earlier `ssh` that reused the pid.
const ATTEMPT_MAX_AGE: Duration = Duration::from_secs(300);

/// What the helper should do with one prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Answer from the keyring.
    UseStoredPassword,
    /// Ask on the terminal with echo: a question such as the host-key check.
    AskVisible,
    /// Ask on the terminal without echo: a passphrase, code or other password.
    AskHidden,
}

/// `user@host` as `ssh -G` resolved it, which is what its prompt will name.
pub fn target_from_config(effective_config: &str) -> Option<String> {
    let value = |key: &str| {
        effective_config.lines().find_map(|line| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '))
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    };
    Some(format!("{}@{}", value("user")?, value("hostname")?))
}

/// Classify a prompt against the expected target. Only an exact
/// `user@host's password: ` is ever answered automatically.
pub fn decide(prompt: &str, target: &str) -> Decision {
    let trimmed = prompt.trim();
    // Keyboard-interactive (PAM) servers ask through OpenSSH's own
    // `(user@host) Password:`, which names the target just as well. Any other
    // text after the prefix is the server's: a code, or a question.
    if !target.is_empty()
        && (trimmed == format!("{}'s password:", target)
            || trimmed == format!("({}) Password:", target))
    {
        return Decision::UseStoredPassword;
    }
    let lower = trimmed.to_lowercase();
    // The host-key prompt asks for yes/no/[fingerprint]; those answers are not
    // secret, and the person has to read what they type.
    if lower.contains("yes/no") || lower.contains("fingerprint") {
        return Decision::AskVisible;
    }
    Decision::AskHidden
}

fn attempts_dir() -> PathBuf {
    paths::config_dir().join("askpass")
}

/// True the first time it is called for a given `ssh` process. A wrong stored
/// password must not be replayed: `ssh` would retry it until the server locks
/// the account, so a second prompt goes to the person instead.
pub fn first_attempt(ssh_pid: u32) -> bool {
    first_attempt_in(&attempts_dir(), ssh_pid)
}

fn first_attempt_in(dir: &Path, ssh_pid: u32) -> bool {
    let builder = {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
    };
    if builder.create(dir).is_err() {
        // Without a marker a retry cannot be detected, so do not answer.
        return false;
    }
    purge_stale(dir);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(ssh_pid.to_string()))
        .is_ok()
}

fn purge_stale(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > ATTEMPT_MAX_AGE);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// ── Keyring ──────────────────────────────────────────────────

pub fn keyring_available() -> bool {
    Command::new("secret-tool")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// The stored password, without its line ending.
pub fn lookup_secret(id: &str) -> Option<String> {
    let output = Command::new("secret-tool")
        .args(["lookup", SECRET_ATTRIBUTE, id])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let secret = String::from_utf8(output.stdout).ok()?;
    let secret = secret.trim_end_matches(['\n', '\r']);
    (!secret.is_empty()).then(|| secret.to_string())
}

pub fn store_secret(id: &str, label: &str, secret: &str) -> Result<(), String> {
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            &format!("--label=Oxterm SSH: {}", label),
            SECRET_ATTRIBUTE,
            id,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run secret-tool: {}", error))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(secret.as_bytes())
            .map_err(|error| format!("cannot write to secret-tool: {}", error))?;
    }
    let status = child
        .wait()
        .map_err(|error| format!("secret-tool failed: {}", error))?;
    if status.success() {
        Ok(())
    } else {
        Err("the system keyring refused the password".to_string())
    }
}

pub fn clear_secret(id: &str) {
    let _ = Command::new("secret-tool")
        .args(["clear", SECRET_ATTRIBUTE, id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Environment that makes `ssh` use the helper for this connection. It holds no
/// secret, so it may appear in the process list.
pub fn ssh_environment(helper: &Path, id: &str, target: &str) -> Vec<(String, String)> {
    vec![
        ("SSH_ASKPASS".to_string(), helper.display().to_string()),
        ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
        (ENV_ID.to_string(), id.to_string()),
        (ENV_TARGET.to_string(), target.to_string()),
    ]
}

/// The helper installed next to `oxterm`, as the broker is.
pub fn helper_path() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.with_file_name(HELPER_NAME);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "admin@10.0.0.1";

    #[test]
    fn only_the_exact_target_password_prompt_is_answered() {
        assert_eq!(
            decide("admin@10.0.0.1's password: ", TARGET),
            Decision::UseStoredPassword
        );
        assert_eq!(
            decide("(admin@10.0.0.1) Password: ", TARGET),
            Decision::UseStoredPassword
        );
    }

    #[test]
    fn a_bastion_never_receives_the_target_password() {
        for prompt in [
            "admin@bastion's password: ",
            "(admin@bastion) Password: ",
            "(admin@10.0.0.1) Verification code: ",
            "root@10.0.0.1's password: ",
            "Password: ",
            "password: ",
            "admin@10.0.0.1.evil.example's password: ",
            "x admin@10.0.0.1's password: ",
        ] {
            assert_eq!(decide(prompt, TARGET), Decision::AskHidden, "{}", prompt);
        }
    }

    #[test]
    fn host_key_questions_are_visible_and_never_answered() {
        let prompt = "The authenticity of host 'h (1.2.3.4)' can't be established.\n\
                      ED25519 key fingerprint is SHA256:abc.\n\
                      Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(decide(prompt, TARGET), Decision::AskVisible);
        assert_eq!(
            decide("Please type 'yes', 'no' or the fingerprint: ", TARGET),
            Decision::AskVisible
        );
    }

    #[test]
    fn passphrases_and_codes_go_to_the_person() {
        for prompt in [
            "Enter passphrase for key '/home/a/.ssh/id': ",
            "Verification code: ",
            "One-time password (OATH) for `admin': ",
        ] {
            assert_eq!(decide(prompt, TARGET), Decision::AskHidden, "{}", prompt);
        }
    }

    #[test]
    fn an_unknown_target_answers_nothing() {
        assert_eq!(decide("'s password: ", ""), Decision::AskHidden);
    }

    #[test]
    fn target_comes_from_the_effective_configuration() {
        let config = "user admin\nhostname 10.0.0.1\nport 22\n";
        assert_eq!(target_from_config(config).as_deref(), Some(TARGET));
        assert_eq!(target_from_config("port 22\n"), None);
    }

    #[test]
    fn a_stored_password_is_offered_only_once_per_ssh() {
        let dir = std::env::temp_dir().join(format!("oxterm-askpass-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(first_attempt_in(&dir, 41));
        assert!(!first_attempt_in(&dir, 41), "a retry goes to the person");
        assert!(first_attempt_in(&dir, 42), "another ssh starts over");
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_environment_carries_no_secret() {
        let env = ssh_environment(Path::new("/x/oxterm-askpass"), "c1", TARGET);
        assert!(env
            .iter()
            .any(|(k, v)| k == "SSH_ASKPASS_REQUIRE" && v == "force"));
        assert!(env.iter().any(|(k, v)| k == ENV_TARGET && v == TARGET));
        assert_eq!(env.len(), 4);
    }
}
