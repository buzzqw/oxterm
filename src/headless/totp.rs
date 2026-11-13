//! One-time codes (RFC 6238) for servers that ask for a verification code.
//!
//! The secret lives in the system keyring and only `oxterm-askpass` reads it.
//! SHA-1 is what the standard and every authenticator app use for these codes;
//! it is implemented here because a code generator is all Oxterm needs from a
//! cryptography library, and the RFC test vectors below pin the result.

use std::time::{SystemTime, UNIX_EPOCH};

const PERIOD: u64 = 30;
const DIGITS: u32 = 6;
const BLOCK: usize = 64;

// `as_chunks` would be nicer, but it needs Rust 1.88 and this crate does not
// declare a minimum toolchain that high yet.
#[allow(clippy::chunks_exact_to_as_chunks)]
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % BLOCK != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((message.len() as u64).wrapping_mul(8)).to_be_bytes());
    for chunk in padded.chunks_exact(BLOCK) {
        let mut words = [0u32; 80];
        for (word, bytes) in words.iter_mut().zip(chunk.chunks_exact(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..80 {
            words[i] = (words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (i, word) in words.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut digest = [0u8; 20];
    for (bytes, word) in digest.chunks_exact_mut(4).zip(state) {
        bytes.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..20].copy_from_slice(&sha1(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|byte| byte ^ 0x36).collect();
    inner.extend_from_slice(message);
    let mut outer: Vec<u8> = block.iter().map(|byte| byte ^ 0x5C).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// Decode the Base32 secret an authenticator setup shows, tolerating the spaces,
/// padding and lower case that people copy along with it.
pub fn decode_secret(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut count = 0u32;
    let mut bytes = Vec::new();
    for c in text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=' && *c != '-')
    {
        let value = match c.to_ascii_uppercase() {
            letter @ 'A'..='Z' => letter as u32 - 'A' as u32,
            digit @ '2'..='7' => digit as u32 - '2' as u32 + 26,
            _ => return None,
        };
        bits = (bits << 5) | value;
        count += 5;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

/// The Base32 secret from what the user pasted: the secret, or an `otpauth://`
/// URI. A URI asking for anything but the 6-digit, 30-second SHA-1 default is
/// refused rather than producing codes the server would reject.
pub fn normalize_secret(input: &str) -> Result<String, String> {
    let input = input.trim();
    let secret = match input.strip_prefix("otpauth://") {
        None => input.to_string(),
        Some(rest) => {
            let query = rest.split_once('?').map_or("", |(_, query)| query);
            let mut secret = None;
            for pair in query.split('&') {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                match key.to_ascii_lowercase().as_str() {
                    "secret" => secret = Some(value.to_string()),
                    "digits" if value != "6" => {
                        return Err("Only 6-digit codes are supported".to_string());
                    }
                    "period" if value != "30" => {
                        return Err("Only 30-second codes are supported".to_string());
                    }
                    "algorithm" if !value.eq_ignore_ascii_case("sha1") => {
                        return Err("Only SHA-1 codes are supported".to_string());
                    }
                    _ => {}
                }
            }
            secret.ok_or("That URI has no secret")?
        }
    };
    if decode_secret(&secret).is_none() {
        return Err("That is not a valid Base32 secret".to_string());
    }
    Ok(secret.split_whitespace().collect())
}

fn code_at(key: &[u8], unix_time: u64) -> String {
    let digest = hmac_sha1(key, &(unix_time / PERIOD).to_be_bytes());
    let offset = (digest[19] & 0x0F) as usize;
    let value = u32::from_be_bytes([
        digest[offset] & 0x7F,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!(
        "{:0width$}",
        value % 10u32.pow(DIGITS),
        width = DIGITS as usize
    )
}

/// The current code for a stored secret.
pub fn current_code(secret: &str) -> Option<String> {
    let key = decode_secret(secret)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(code_at(&key, now))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
    }

    #[test]
    fn sha1_matches_the_standard_vectors() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn hmac_matches_rfc_2202() {
        assert_eq!(
            hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        assert_eq!(
            hex(&hmac_sha1(
                &[0xaa; 80],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "aa4ae5e15272d00e95705637ce8a3b55ed402112"
        );
    }

    #[test]
    fn codes_match_rfc_6238() {
        // The RFC's 8-digit values, reduced to the last six.
        let key = b"12345678901234567890";
        for (time, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
        ] {
            assert_eq!(code_at(key, time), code);
        }
    }

    #[test]
    fn secrets_are_decoded_forgivingly_and_refused_when_wrong() {
        assert_eq!(decode_secret("GEZD GNBV").unwrap(), b"12345");
        assert_eq!(decode_secret("gezdgnbv==").unwrap(), b"12345");
        assert!(decode_secret("not base32!").is_none());
        assert!(decode_secret("").is_none());
    }

    #[test]
    fn otpauth_uris_give_their_secret_or_a_clear_refusal() {
        assert_eq!(normalize_secret("GEZD GNBV").unwrap(), "GEZDGNBV");
        assert_eq!(
            normalize_secret("otpauth://totp/x?secret=GEZDGNBV&issuer=y&period=30").unwrap(),
            "GEZDGNBV"
        );
        for bad in [
            "otpauth://totp/x?issuer=y",
            "otpauth://totp/x?secret=GEZDGNBV&digits=8",
            "otpauth://totp/x?secret=GEZDGNBV&period=60",
            "otpauth://totp/x?secret=GEZDGNBV&algorithm=SHA256",
            "!!!",
        ] {
            assert!(normalize_secret(bad).is_err(), "{}", bad);
        }
    }
}
