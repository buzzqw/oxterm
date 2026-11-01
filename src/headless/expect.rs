//! Rules that type a command when a pattern appears in a terminal.
//!
//! The window feeds in the text near the cursor and gets back what to send. A
//! prompt stays on screen after it is answered, so each rule remembers what it
//! matched: without that, any later output would answer the same prompt again.

use regex::Regex;

use super::connections::ExpectRule;

/// A rule that reads the screen and types is a loop if it can match its own
/// output, so the total is bounded even when a rule asks for more.
const MAX_TRIGGERS: u32 = 100;

struct Compiled {
    pattern: Regex,
    command: String,
    remaining: u32,
    delay_ms: u64,
    last_match: Option<String>,
}

pub struct Watcher {
    rules: Vec<Compiled>,
}

/// A command to type, after a delay.
#[derive(Debug, PartialEq, Eq)]
pub struct Send {
    pub command: String,
    pub delay_ms: u64,
}

impl Watcher {
    /// Rules that do not compile are dropped; the form refuses them when saved.
    pub fn new(rules: &[ExpectRule]) -> Watcher {
        Watcher {
            rules: rules
                .iter()
                .filter_map(|rule| {
                    Some(Compiled {
                        pattern: Regex::new(&rule.pattern).ok()?,
                        command: rule.command.clone(),
                        remaining: rule.max_trigger.clamp(1, MAX_TRIGGERS),
                        delay_ms: rule.delay_ms,
                        last_match: None,
                    })
                })
                .collect(),
        }
    }

    /// True when no rule can fire any more, so the caller can stop watching.
    pub fn finished(&self) -> bool {
        self.rules.iter().all(|rule| rule.remaining == 0)
    }

    /// What to send for the text now near the cursor.
    pub fn check(&mut self, tail: &str) -> Vec<Send> {
        let mut sends = Vec::new();
        for rule in &mut self.rules {
            if rule.remaining == 0 {
                continue;
            }
            let Some(found) = rule.pattern.find(tail) else {
                continue;
            };
            // Everything up to the match: the same prompt on screen is the same
            // text, a new prompt further down is not.
            let key = &tail[..found.end()];
            if rule.last_match.as_deref() == Some(key) {
                continue;
            }
            rule.last_match = Some(key.to_string());
            rule.remaining -= 1;
            sends.push(Send {
                command: rule.command.clone(),
                delay_ms: rule.delay_ms,
            });
        }
        sends
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, command: &str, max_trigger: u32) -> ExpectRule {
        ExpectRule {
            pattern: pattern.to_string(),
            command: command.to_string(),
            max_trigger,
            delay_ms: 10,
        }
    }

    #[test]
    fn a_prompt_is_answered_once_however_often_the_screen_changes() {
        let mut watcher = Watcher::new(&[rule("Continue\\?", "yes", 5)]);
        assert_eq!(watcher.check("Continue?").len(), 1);
        assert!(watcher.check("Continue?").is_empty());
        assert!(watcher.check("Continue? yes").is_empty());
    }

    #[test]
    fn a_new_prompt_counts_until_the_limit() {
        let mut watcher = Watcher::new(&[rule("next>", "go", 2)]);
        assert_eq!(watcher.check("a next>").len(), 1);
        assert!(!watcher.finished());
        assert_eq!(
            watcher.check("a next> go\nb next>").len(),
            0,
            "first match is unchanged"
        );
        assert_eq!(watcher.check("b next>").len(), 1);
        assert!(watcher.finished());
        assert!(watcher.check("c next>").is_empty());
    }

    #[test]
    fn rules_are_independent_and_bad_patterns_are_dropped() {
        let mut watcher = Watcher::new(&[rule("[", "x", 1), rule("a", "1", 1), rule("b", "2", 1)]);
        let sends = watcher.check("a b");
        assert_eq!(
            sends
                .iter()
                .map(|send| send.command.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(sends[0].delay_ms, 10);
    }

    #[test]
    fn no_rules_means_nothing_to_watch() {
        assert!(Watcher::new(&[]).finished());
    }
}
