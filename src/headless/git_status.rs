//! Compact Git status for the active tab, computed without a shell.
//!
//! Only `git status --porcelain=v2 --branch` is needed: the output is stable,
//! easy to parse and does not depend on the user's colour or format settings.
//! The badge is display-only; it never changes the repository.

use std::path::Path;
use std::time::Duration;

/// The counts shown by the badge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: String,
    pub staged: usize,
    pub changed: usize,
    pub untracked: usize,
    pub ahead: usize,
    pub behind: usize,
}

impl GitStatus {
    /// One short line: `branch +2 ~3 ?1 ↑2`. No Nerd Font is required.
    pub fn compact(&self) -> String {
        let mut parts = vec![self.branch.clone()];
        if self.staged > 0 {
            parts.push(format!("+{}", self.staged));
        }
        if self.changed > 0 {
            parts.push(format!("~{}", self.changed));
        }
        if self.untracked > 0 {
            parts.push(format!("?{}", self.untracked));
        }
        if self.ahead > 0 {
            parts.push(format!("↑{}", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("↓{}", self.behind));
        }
        parts.join(" ")
    }

    /// Full text for the tooltip.
    pub fn summary(&self) -> String {
        format!(
            "{}: {} staged, {} changed, {} untracked, {} ahead, {} behind",
            self.branch, self.staged, self.changed, self.untracked, self.ahead, self.behind
        )
    }
}

fn leading_number(value: &str) -> usize {
    let digits: String = value
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().unwrap_or(0)
}

fn count_word(line: &str, word: &str) -> usize {
    line.split_once(word)
        .map(|(_, rest)| leading_number(rest))
        .unwrap_or(0)
}

/// Parse `git status --porcelain=v2 --branch`. Unrecognised lines are ignored:
/// a future Git version may add fields without breaking the badge.
pub fn parse(output: &str) -> Option<GitStatus> {
    let mut status = GitStatus::default();
    let mut seen_branch = false;
    for line in output.lines() {
        // Porcelain v2 uses header lines. `# branch.ab +2 -1` is the
        // authoritative ahead/behind count; v1 puts it in the `##` line.
        if let Some(head) = line.strip_prefix("# branch.head ") {
            seen_branch = true;
            let name = head.trim();
            status.branch = if name == "(detached)" {
                "HEAD".to_string()
            } else {
                name.to_string()
            };
            continue;
        }
        if let Some(head) = line.strip_prefix("# branch.ab ") {
            let mut words = head.split_whitespace();
            status.ahead = words
                .next()
                .and_then(|value| value.strip_prefix('+'))
                .map(leading_number)
                .unwrap_or(0);
            status.behind = words
                .next()
                .and_then(|value| value.strip_prefix('-'))
                .map(leading_number)
                .unwrap_or(0);
            continue;
        }
        if line.starts_with("# ") {
            continue;
        }
        if let Some(head) = line.strip_prefix("## ") {
            seen_branch = true;
            // `## main...origin/main [ahead 2, behind 1]`; the part after the
            // first `...` is the upstream and may be absent.
            let name = head.split("...").next().unwrap_or(head);
            let name = name.split_whitespace().next().unwrap_or("");
            status.branch = name.to_string();
            status.ahead = count_word(head, "ahead ");
            status.behind = count_word(head, "behind ");
            continue;
        }
        if let Some(rest) = line.strip_prefix("1 ") {
            // `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`; X is the index
            // state, Y the worktree state. '.' means clean for that side.
            let mut words = rest.split_whitespace();
            let xy = words.next().unwrap_or("");
            let mut chars = xy.chars();
            let index = chars.next().unwrap_or('.');
            let worktree = chars.next().unwrap_or('.');
            if index != '.' {
                status.staged += 1;
            }
            if worktree != '.' {
                status.changed += 1;
            }
        } else if line.starts_with("? ") {
            status.untracked += 1;
        } else if line.starts_with("u ") || line.starts_with("2 ") {
            status.changed += 1;
        }
    }
    if seen_branch && !status.branch.is_empty() {
        Some(status)
    } else {
        None
    }
}

/// Ask git about one directory. Non-zero exits (not a repository, no git) mean
/// "no badge" rather than an error dialog.
pub fn snapshot(cwd: &Path, timeout: Duration) -> Option<GitStatus> {
    if !cwd.is_dir() {
        return None;
    }
    let output =
        super::extensions::run_command("git status --porcelain=v2 --branch", Some(cwd), timeout)
            .ok()?;
    parse(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_porcelain_v2_output() {
        let output = "# branch.oid abc\n\
                      # branch.head main\n\
                      # branch.upstream origin/main\n\
                      # branch.ab +2 -1\n\
                      1 M. N... 100644 100644 100644 abc abc file\n\
                      1 .M N... 100644 100644 100644 abc abc other\n\
                      ? new.txt\n";
        let status = parse(output).unwrap();
        assert_eq!(status.branch, "main");
        assert_eq!(status.staged, 1);
        assert_eq!(status.changed, 1);
        assert_eq!(status.untracked, 1);
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 1);
        assert_eq!(status.compact(), "main +1 ~1 ?1 ↑2 ↓1");
        assert!(status.summary().contains("main"));

        // Porcelain v1 -b format is still understood for other callers.
        let old = parse("## main...origin/main [ahead 2, behind 1]\n").unwrap();
        assert_eq!(old.ahead, 2);
        assert_eq!(old.behind, 1);
    }

    #[test]
    fn snapshot_reads_the_current_repository() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        if !root.join(".git").exists() {
            return; // a source tarball has no repository to read
        }
        let status = snapshot(root, Duration::from_secs(2)).unwrap();
        assert!(!status.branch.is_empty());
        assert!(!status.compact().is_empty());
    }

    #[test]
    fn a_detached_head_or_plain_branch_still_makes_a_badge() {
        let status = parse("# branch.head (detached)\n").unwrap();
        assert_eq!(status.branch, "HEAD");
        assert_eq!(status.compact(), "HEAD");
        let old = parse("## HEAD (no branch)\n").unwrap();
        assert_eq!(old.branch, "HEAD");
        assert!(parse("garbage").is_none());
        assert!(parse("").is_none());
    }
}
