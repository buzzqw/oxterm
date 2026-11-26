//! Password and passphrase generation, kept display-free so it can be used from
//! the CLI and tested without a GUI.
//!
//! Randomness comes from `getrandom`, with `/dev/urandom` as a fallback. If
//! neither is available the call fails: a predictable password is worse than
//! no password at all.

use std::fs::File;
use std::io::Read;

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*()-_=+[]{};:,.?/";

/// Short, unambiguous words: no `0/O`, `1/l/I` and no longer than four letters.
const WORDS: [&str; 256] = [
    "ace", "age", "ago", "ape", "apt", "arm", "art", "ate", "aunt", "baby", "bag", "ban", "base",
    "bath", "bean", "beat", "bend", "best", "body", "book", "born", "box", "brag", "bug", "burn",
    "busy", "cab", "can", "cap", "card", "cash", "cat", "chat", "chew", "code", "come", "copy",
    "core", "cost", "crab", "crew", "cry", "cuff", "cut", "dad", "dash", "date", "dead", "debt",
    "deed", "den", "desk", "dog", "done", "dose", "dove", "draw", "drug", "dry", "due", "dune",
    "each", "earn", "easy", "echo", "end", "era", "ever", "face", "fade", "fan", "far", "fast",
    "fate", "feat", "feed", "fest", "foam", "fond", "ford", "form", "fox", "frog", "fund", "fury",
    "gang", "gate", "gear", "gene", "get", "gone", "gown", "gray", "grow", "gun", "guts", "gym",
    "hard", "harm", "hat", "have", "haze", "heap", "hen", "herd", "hero", "hood", "hook", "hope",
    "hose", "hour", "hub", "hump", "hunt", "husk", "jade", "jar", "jeep", "jet", "joy", "junk",
    "keen", "kept", "knew", "know", "made", "mane", "map", "mars", "mass", "mate", "may", "mean",
    "meet", "mend", "mess", "moan", "mode", "monk", "more", "most", "much", "muse", "mute", "nap",
    "near", "need", "nets", "node", "noon", "noun", "odds", "onto", "ouch", "oven", "owes", "pack",
    "pane", "pant", "pass", "path", "pear", "peck", "pens", "poem", "poke", "poor", "pore", "pose",
    "pour", "prod", "pure", "putt", "raft", "rake", "rare", "rave", "rear", "reed", "rent", "roam",
    "robe", "roof", "root", "rosy", "rout", "rude", "rush", "sage", "sand", "sang", "scan", "seam",
    "seek", "seen", "shed", "shop", "shut", "snow", "sock", "soft", "song", "sore", "soup", "span",
    "spun", "stag", "stay", "stew", "stub", "such", "sure", "swam", "sway", "tact", "tank", "task",
    "tend", "test", "than", "them", "they", "toe", "tone", "tore", "toss", "town", "tray", "trek",
    "tube", "tune", "tusk", "undo", "urge", "user", "vast", "vent", "vest", "wade", "wake", "ward",
    "warn", "wash", "wave", "weed", "weep", "west", "whom", "word", "worm", "wrap", "yarn", "yeah",
    "yoke", "zero", "zoom",
];

fn random_bytes(buf: &mut [u8]) -> Result<(), String> {
    let mut filled = 0;
    while filled < buf.len() {
        let written =
            unsafe { libc::getrandom(buf[filled..].as_mut_ptr().cast(), buf[filled..].len(), 0) };
        if written > 0 {
            filled += written as usize;
        } else {
            break;
        }
    }
    if filled == buf.len() {
        return Ok(());
    }
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut buf[filled..]))
        .map_err(|error| format!("Cannot obtain random bytes: {}", error))
}

/// A small buffered source of unbiased indices: bytes that would bias a modulo
/// reduction are discarded instead of being reused.
struct Random {
    bytes: Vec<u8>,
    at: usize,
}

impl Random {
    fn new() -> Result<Self, String> {
        let mut bytes = vec![0u8; 256];
        random_bytes(&mut bytes)?;
        Ok(Self { bytes, at: 0 })
    }

    fn byte(&mut self) -> Result<u8, String> {
        if self.at == self.bytes.len() {
            random_bytes(&mut self.bytes)?;
            self.at = 0;
        }
        let byte = self.bytes[self.at];
        self.at += 1;
        Ok(byte)
    }

    fn index(&mut self, len: usize) -> Result<usize, String> {
        let limit = 256 - (256 % len);
        loop {
            let byte = self.byte()? as usize;
            if byte < limit {
                return Ok(byte % len);
            }
        }
    }
}

fn pick(rng: &mut Random, alphabet: &str) -> Result<char, String> {
    let bytes = alphabet.as_bytes();
    Ok(bytes[rng.index(bytes.len())?] as char)
}

fn round_tenths(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Generate a password of 8 to 128 characters, with at least one lowercase
/// letter, one uppercase letter, one digit and (when requested) one symbol.
pub fn generate_password(length: usize, symbols: bool) -> Result<String, String> {
    if !(8..=128).contains(&length) {
        return Err("Password length must be between 8 and 128 characters".to_string());
    }
    let mut alphabet = String::with_capacity(82);
    alphabet.push_str(LOWER);
    alphabet.push_str(UPPER);
    alphabet.push_str(DIGITS);
    if symbols {
        alphabet.push_str(SYMBOLS);
    }
    let mut required = vec![LOWER, UPPER, DIGITS];
    if symbols {
        required.push(SYMBOLS);
    }

    let mut rng = Random::new()?;
    let mut chars: Vec<char> = Vec::with_capacity(length);
    for class in required {
        chars.push(pick(&mut rng, class)?);
    }
    while chars.len() < length {
        chars.push(pick(&mut rng, &alphabet)?);
    }
    for i in (1..chars.len()).rev() {
        let j = rng.index(i + 1)?;
        chars.swap(i, j);
    }
    Ok(chars.into_iter().collect())
}

/// Generate a passphrase of 3 to 12 hyphen-separated words and return it with
/// its entropy in bits, rounded to one decimal.
pub fn generate_passphrase(words: usize) -> Result<(String, f64), String> {
    if !(3..=12).contains(&words) {
        return Err("Passphrase length must be between 3 and 12 words".to_string());
    }
    let mut rng = Random::new()?;
    let chosen: Vec<&str> = (0..words)
        .map(|_| Ok(WORDS[rng.index(WORDS.len())?]))
        .collect::<Result<_, String>>()?;
    Ok((
        chosen.join("-"),
        round_tenths(words as f64 * (WORDS.len() as f64).log2()),
    ))
}

/// Entropy of a uniformly chosen password of `length` characters, in bits.
pub fn entropy_bits(length: usize, symbols: bool) -> f64 {
    let alphabet = 62.0 + if symbols { SYMBOLS.len() as f64 } else { 0.0 };
    length as f64 * alphabet.log2()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(chars: &str, class: &str) -> bool {
        chars.chars().any(|c| class.contains(c))
    }

    #[test]
    fn password_lengths_and_bounds() {
        assert_eq!(generate_password(8, false).unwrap().len(), 8);
        assert_eq!(generate_password(20, true).unwrap().len(), 20);
        assert_eq!(generate_password(128, false).unwrap().len(), 128);
        for length in [0, 1, 7, 129, usize::MAX] {
            assert!(generate_password(length, false).is_err());
        }
    }

    #[test]
    fn password_contains_every_required_class() {
        for _ in 0..40 {
            let plain = generate_password(16, false).unwrap();
            assert!(has(&plain, LOWER));
            assert!(has(&plain, UPPER));
            assert!(has(&plain, DIGITS));
            assert!(!has(&plain, SYMBOLS));
            let with_symbols = generate_password(16, true).unwrap();
            assert!(has(&with_symbols, LOWER));
            assert!(has(&with_symbols, UPPER));
            assert!(has(&with_symbols, DIGITS));
            assert!(has(&with_symbols, SYMBOLS));
        }
    }

    #[test]
    fn passwords_are_not_all_the_same() {
        let passwords: Vec<String> = (0..12)
            .map(|_| generate_password(24, true).unwrap())
            .collect();
        let mut unique = passwords.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), passwords.len());
    }

    #[test]
    fn passphrase_words_and_bounds() {
        for words in 3..=12 {
            let (phrase, entropy) = generate_passphrase(words).unwrap();
            let parts: Vec<&str> = phrase.split('-').collect();
            assert_eq!(parts.len(), words);
            assert!(parts.iter().all(|word| WORDS.contains(word)));
            assert_eq!(entropy, round_tenths(words as f64 * 8.0));
        }
        for words in [0, 1, 2, 13, usize::MAX] {
            assert!(generate_passphrase(words).is_err());
        }
    }

    #[test]
    fn word_list_is_short_and_unambiguous() {
        assert!(WORDS.len() >= 256);
        let mut unique = WORDS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), WORDS.len());
        for word in WORDS {
            assert!(word.len() <= 5, "{}", word);
            assert!(!word.chars().any(|c| "01lIO".contains(c)), "{}", word);
        }
    }

    #[test]
    fn entropy_estimates_are_repeatable() {
        assert_eq!(entropy_bits(12, false), 12.0 * 62.0_f64.log2());
        assert_eq!(
            entropy_bits(12, true),
            12.0 * (62.0 + SYMBOLS.len() as f64).log2()
        );
        assert!(entropy_bits(12, true) > entropy_bits(12, false));
        assert!(entropy_bits(16, false) > entropy_bits(12, false));
        assert_eq!(entropy_bits(0, false), 0.0);
    }
}
