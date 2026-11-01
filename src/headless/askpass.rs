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
pub const ENV_TOTP: &str = "OXTERM_ASKPASS_TOTP";
pub const HELPER_NAME: &str = "oxterm-askpass";

const SECRET_ATTRIBUTE: &str = "oxterm-connection";
const TOTP_ATTRIBUTE: &str = "oxterm-connection-totp";
/// A marker this old belongs to an earlier `ssh` that reused the pid.
const ATTEMPT_MAX_AGE: Duration = Duration::from_secs(300);

/// What the helper should do with one prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Answer from the keyring.
    UseStoredPassword,
    /// Answer with a code computed from the secret in the keyring.
    UseTotpCode,
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

/// Classify a prompt against the expected target. Only a prompt that names the
/// target is ever answered automatically.
pub fn decide(prompt: &str, target: &str, totp: bool) -> Decision {
    let trimmed = prompt.trim();
    // OpenSSH prefixes what a server asks with `(user@host)`, so this is bound to
    // the target just as the password prompt is. A bastion's code prompt names
    // the bastion, and the secret is not its to receive.
    if totp && !target.is_empty() {
        if let Some(question) = trimmed.strip_prefix(&format!("({}) ", target)) {
            let question = question.to_lowercase();
            if [
                "verification code",
                "one-time",
                "otp",
                "authenticator",
                "token",
            ]
            .iter()
            .any(|word| question.contains(word))
            {
                return Decision::UseTotpCode;
            }
        }
    }
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
pub fn first_attempt(ssh_pid: u32, kind: &str) -> bool {
    first_attempt_in(&attempts_dir(), &format!("{}-{}", ssh_pid, kind))
}

fn first_attempt_in(dir: &Path, marker: &str) -> bool {
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
        .open(dir.join(marker))
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
    lookup(SECRET_ATTRIBUTE, id)
}

/// The current one-time code, computed from the stored secret.
pub fn lookup_totp_code(id: &str) -> Option<String> {
    super::totp::current_code(&lookup(TOTP_ATTRIBUTE, id)?)
}

fn lookup(attribute: &str, id: &str) -> Option<String> {
    let output = Command::new("secret-tool")
        .args(["lookup", attribute, id])
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
    store(
        SECRET_ATTRIBUTE,
        id,
        &format!("Oxterm SSH: {}", label),
        secret,
    )
}

pub fn store_totp_secret(id: &str, label: &str, secret: &str) -> Result<(), String> {
    store(
        TOTP_ATTRIBUTE,
        id,
        &format!("Oxterm SSH one-time code: {}", label),
        secret,
    )
}

fn store(attribute: &str, id: &str, label: &str, secret: &str) -> Result<(), String> {
    let mut child = Command::new("secret-tool")
        .args(["store", &format!("--label={}", label), attribute, id])
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
    clear(SECRET_ATTRIBUTE, id);
}

pub fn clear_totp_secret(id: &str) {
    clear(TOTP_ATTRIBUTE, id);
}

fn clear(attribute: &str, id: &str) {
    let _ = Command::new("secret-tool")
        .args(["clear", attribute, id])
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
            decide("admin@10.0.0.1's password: ", TARGET, false),
            Decision::UseStoredPassword
        );
        assert_eq!(
            decide("(admin@10.0.0.1) Password: ", TARGET, false),
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
            assert_eq!(
                decide(prompt, TARGET, false),
                Decision::AskHidden,
                "{}",
                prompt
            );
        }
    }

    #[test]
    fn host_key_questions_are_visible_and_never_answered() {
        let prompt = "The authenticity of host 'h (1.2.3.4)' can't be established.\n\
                      ED25519 key fingerprint is SHA256:abc.\n\
                      Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(decide(prompt, TARGET, false), Decision::AskVisible);
        assert_eq!(
            decide(
                "Please type 'yes', 'no' or the fingerprint: ",
                TARGET,
                false
            ),
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
            assert_eq!(
                decide(prompt, TARGET, false),
                Decision::AskHidden,
                "{}",
                prompt
            );
        }
    }

    #[test]
    fn a_code_goes_only_to_the_target_and_only_when_enabled() {
        let prompt = "(admin@10.0.0.1) Verification code: ";
        assert_eq!(decide(prompt, TARGET, true), Decision::UseTotpCode);
        assert_eq!(decide(prompt, TARGET, false), Decision::AskHidden);
        for other in [
            "(admin@bastion) Verification code: ",
            "Verification code: ",
            "(admin@10.0.0.1) Enter your favorite color: ",
        ] {
            assert_eq!(
                decide(other, TARGET, true),
                Decision::AskHidden,
                "{}",
                other
            );
        }
        assert_eq!(
            decide("(admin@10.0.0.1) Password: ", TARGET, true),
            Decision::UseStoredPassword
        );
    }

    #[test]
    fn an_unknown_target_answers_nothing() {
        assert_eq!(decide("'s password: ", "", false), Decision::AskHidden);
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
        assert!(first_attempt_in(&dir, "41-password"));
        assert!(
            !first_attempt_in(&dir, "41-password"),
            "a retry goes to the person"
        );
        assert!(
            first_attempt_in(&dir, "42-password"),
            "another ssh starts over"
        );
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
