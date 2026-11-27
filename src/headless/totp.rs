//! One-time codes (RFC 6238) for servers that ask for a verification code.
//!
//! The secret lives in the system keyring and only `oxterm-askpass` reads it.
//! SHA-1 is the default, but SHA-256 and SHA-512 are supported as well for
//! providers whose `otpauth://` URI asks for them. The hashes are implemented
//! here because a code generator is all Oxterm needs from a cryptography
//! library, and the RFC test vectors below pin the result.

use std::time::{SystemTime, UNIX_EPOCH};

const BLOCK_64: usize = 64;
const BLOCK_128: usize = 128;

/// The hash an `otpauth://` provider asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotpAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

/// The parameters of a TOTP code: digits, time step and hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TotpParams {
    pub digits: u32,
    pub period: u64,
    pub algorithm: TotpAlgorithm,
}

impl Default for TotpParams {
    fn default() -> Self {
        Self {
            digits: 6,
            period: 30,
            algorithm: TotpAlgorithm::Sha1,
        }
    }
}

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
    for chunk in pad(message, BLOCK_64, 8).chunks_exact(BLOCK_64) {
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

const SHA256_K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

fn sha256(message: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    for chunk in pad(message, BLOCK_64, 8).as_chunks::<BLOCK_64>().0 {
        let mut words = [0u32; 64];
        for (word, bytes) in words.iter_mut().zip(chunk.as_chunks::<4>().0) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..64 {
            let s0 = words[i - 15].rotate_right(7)
                ^ words[i - 15].rotate_right(18)
                ^ (words[i - 15] >> 3);
            let s1 = words[i - 2].rotate_right(17)
                ^ words[i - 2].rotate_right(19)
                ^ (words[i - 2] >> 10);
            words[i] = words[i - 16]
                .wrapping_add(s0)
                .wrapping_add(words[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (i, word) in words.iter().enumerate() {
            let big_s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(big_s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(*word);
            let big_s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = big_s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut digest = [0u8; 32];
    for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        bytes.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

const SHA512_K: [u64; 80] = [
    0x428a_2f98_d728_ae22,
    0x7137_4491_23ef_65cd,
    0xb5c0_fbcf_ec4d_3b2f,
    0xe9b5_dba5_8189_dbbc,
    0x3956_c25b_f348_b538,
    0x59f1_11f1_b605_d019,
    0x923f_82a4_af19_4f9b,
    0xab1c_5ed5_da6d_8118,
    0xd807_aa98_a303_0242,
    0x1283_5b01_4570_6fbe,
    0x2431_85be_4ee4_b28c,
    0x550c_7dc3_d5ff_b4e2,
    0x72be_5d74_f27b_896f,
    0x80de_b1fe_3b16_96b1,
    0x9bdc_06a7_25c7_1235,
    0xc19b_f174_cf69_2694,
    0xe49b_69c1_9ef1_4ad2,
    0xefbe_4786_384f_25e3,
    0x0fc1_9dc6_8b8c_d5b5,
    0x240c_a1cc_77ac_9c65,
    0x2de9_2c6f_592b_0275,
    0x4a74_84aa_6ea6_e483,
    0x5cb0_a9dc_bd41_fbd4,
    0x76f9_88da_8311_53b5,
    0x983e_5152_ee66_dfab,
    0xa831_c66d_2db4_3210,
    0xb003_27c8_98fb_213f,
    0xbf59_7fc7_beef_0ee4,
    0xc6e0_0bf3_3da8_8fc2,
    0xd5a7_9147_930a_a725,
    0x06ca_6351_e003_826f,
    0x1429_2967_0a0e_6e70,
    0x27b7_0a85_46d2_2ffc,
    0x2e1b_2138_5c26_c926,
    0x4d2c_6dfc_5ac4_2aed,
    0x5338_0d13_9d95_b3df,
    0x650a_7354_8baf_63de,
    0x766a_0abb_3c77_b2a8,
    0x81c2_c92e_47ed_aee6,
    0x9272_2c85_1482_353b,
    0xa2bf_e8a1_4cf1_0364,
    0xa81a_664b_bc42_3001,
    0xc24b_8b70_d0f8_9791,
    0xc76c_51a3_0654_be30,
    0xd192_e819_d6ef_5218,
    0xd699_0624_5565_a910,
    0xf40e_3585_5771_202a,
    0x106a_a070_32bb_d1b8,
    0x19a4_c116_b8d2_d0c8,
    0x1e37_6c08_5141_ab53,
    0x2748_774c_df8e_eb99,
    0x34b0_bcb5_e19b_48a8,
    0x391c_0cb3_c5c9_5a63,
    0x4ed8_aa4a_e341_8acb,
    0x5b9c_ca4f_7763_e373,
    0x682e_6ff3_d6b2_b8a3,
    0x748f_82ee_5def_b2fc,
    0x78a5_636f_4317_2f60,
    0x84c8_7814_a1f0_ab72,
    0x8cc7_0208_1a64_39ec,
    0x90be_fffa_2363_1e28,
    0xa450_6ceb_de82_bde9,
    0xbef9_a3f7_b2c6_7915,
    0xc671_78f2_e372_532b,
    0xca27_3ece_ea26_619c,
    0xd186_b8c7_21c0_c207,
    0xeada_7dd6_cde0_eb1e,
    0xf57d_4f7f_ee6e_d178,
    0x06f0_67aa_7217_6fba,
    0x0a63_7dc5_a2c8_98a6,
    0x113f_9804_bef9_0dae,
    0x1b71_0b35_131c_471b,
    0x28db_77f5_2304_7d84,
    0x32ca_ab7b_40c7_2493,
    0x3c9e_be0a_15c9_bebc,
    0x431d_67c4_9c10_0d4c,
    0x4cc5_d4be_cb3e_42b6,
    0x597f_299c_fc65_7e2a,
    0x5fcb_6fab_3ad6_faec,
    0x6c44_198c_4a47_5817,
];

fn sha512(message: &[u8]) -> [u8; 64] {
    let mut state: [u64; 8] = [
        0x6a09_e667_f3bc_c908,
        0xbb67_ae85_84ca_a73b,
        0x3c6e_f372_fe94_f82b,
        0xa54f_f53a_5f1d_36f1,
        0x510e_527f_ade6_82d1,
        0x9b05_688c_2b3e_6c1f,
        0x1f83_d9ab_fb41_bd6b,
        0x5be0_cd19_137e_2179,
    ];
    for chunk in pad(message, BLOCK_128, 16).as_chunks::<BLOCK_128>().0 {
        let mut words = [0u64; 80];
        for (word, bytes) in words.iter_mut().zip(chunk.as_chunks::<8>().0) {
            *word = u64::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
        }
        for i in 16..80 {
            let s0 = words[i - 15].rotate_right(1)
                ^ words[i - 15].rotate_right(8)
                ^ (words[i - 15] >> 7);
            let s1 =
                words[i - 2].rotate_right(19) ^ words[i - 2].rotate_right(61) ^ (words[i - 2] >> 6);
            words[i] = words[i - 16]
                .wrapping_add(s0)
                .wrapping_add(words[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (i, word) in words.iter().enumerate() {
            let big_s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(big_s1)
                .wrapping_add(ch)
                .wrapping_add(SHA512_K[i])
                .wrapping_add(*word);
            let big_s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = big_s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut digest = [0u8; 64];
    for (bytes, word) in digest.as_chunks_mut::<8>().0.iter_mut().zip(state) {
        bytes.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// SHA-1 and SHA-256 use a 64-byte length trailer, SHA-512 uses 128 bits.
fn pad(message: &[u8], block: usize, length_bytes: usize) -> Vec<u8> {
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % block != block - length_bytes {
        padded.push(0);
    }
    let bits = (message.len() as u128).wrapping_mul(8);
    padded.extend_from_slice(&bits.to_be_bytes()[16 - length_bytes..]);
    padded
}

fn hmac(hash: fn(&[u8]) -> Vec<u8>, block: usize, key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut key_block = vec![0u8; block];
    if key.len() > block {
        let digest = hash(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = key_block.iter().map(|byte| byte ^ 0x36).collect();
    inner.extend_from_slice(message);
    let mut outer: Vec<u8> = key_block.iter().map(|byte| byte ^ 0x5C).collect();
    outer.extend_from_slice(&hash(&inner));
    hash(&outer)
}

fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let digest = hmac(|m| sha1(m).to_vec(), BLOCK_64, key, message);
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest);
    out
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let digest = hmac(|m| sha256(m).to_vec(), BLOCK_64, key, message);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn hmac_sha512(key: &[u8], message: &[u8]) -> [u8; 64] {
    let digest = hmac(|m| sha512(m).to_vec(), BLOCK_128, key, message);
    let mut out = [0u8; 64];
    out.copy_from_slice(&digest);
    out
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

/// Parse a bare Base32 secret or an `otpauth://totp/...` URI, returning the
/// secret and the parameters the provider asked for.
pub fn parse_otpauth_uri(input: &str) -> Result<(String, TotpParams), String> {
    let input = input.trim();
    let mut params = TotpParams::default();
    let secret = match input.strip_prefix("otpauth://") {
        None => input.to_string(),
        Some(rest) => {
            if !rest.to_ascii_lowercase().starts_with("totp/") {
                return Err("Only TOTP URIs are supported".to_string());
            }
            let query = rest.split_once('?').map_or("", |(_, query)| query);
            let mut secret = None;
            for pair in query.split('&') {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                match key.to_ascii_lowercase().as_str() {
                    "secret" => secret = Some(value.to_string()),
                    "digits" => {
                        params.digits = value.parse().map_err(|_| "Invalid digits value")?;
                        if !(6..=8).contains(&params.digits) {
                            return Err("digits must be between 6 and 8".to_string());
                        }
                    }
                    "period" => {
                        params.period = value.parse().map_err(|_| "Invalid period value")?;
                        if !(1..=300).contains(&params.period) {
                            return Err("period must be between 1 and 300 seconds".to_string());
                        }
                    }
                    "algorithm" => {
                        params.algorithm = match value.to_ascii_uppercase().as_str() {
                            "SHA1" => TotpAlgorithm::Sha1,
                            "SHA256" => TotpAlgorithm::Sha256,
                            "SHA512" => TotpAlgorithm::Sha512,
                            _ => {
                                return Err(format!("Unsupported TOTP algorithm: {}", value));
                            }
                        };
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
    Ok((secret.split_whitespace().collect(), params))
}

/// The Base32 secret from what the user pasted: the secret, or an `otpauth://`
/// URI. Valid URI parameters are accepted (they are used by `current_code_with`).
pub fn normalize_secret(input: &str) -> Result<String, String> {
    parse_otpauth_uri(input).map(|(secret, _)| secret)
}

fn code_at(key: &[u8], unix_time: u64, params: TotpParams) -> String {
    let counter = (unix_time / params.period).to_be_bytes();
    let digest = match params.algorithm {
        TotpAlgorithm::Sha1 => hmac_sha1(key, &counter).to_vec(),
        TotpAlgorithm::Sha256 => hmac_sha256(key, &counter).to_vec(),
        TotpAlgorithm::Sha512 => hmac_sha512(key, &counter).to_vec(),
    };
    let offset = (digest[digest.len() - 1] & 0x0F) as usize;
    let value = u32::from_be_bytes([
        digest[offset] & 0x7F,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!(
        "{:0width$}",
        value % 10u32.pow(params.digits),
        width = params.digits as usize
    )
}

/// The current code for a stored secret, with the provider's parameters.
pub fn current_code_with(secret: &str, params: TotpParams) -> Option<String> {
    let key = decode_secret(secret)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(code_at(&key, now, params))
}

/// The current 6-digit SHA-1 code for a stored secret.
pub fn current_code(secret: &str) -> Option<String> {
    current_code_with(secret, TotpParams::default())
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
    fn sha256_matches_fips_180_4() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha512_matches_fips_180_4() {
        assert_eq!(
            hex(&sha512(b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hex(&sha512(b"")),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
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
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 20], &[0xdd; 50])),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
        assert_eq!(
            hex(&hmac_sha512(b"Jefe", b"what do ya want for nothing?")),
            "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea250554\
             9758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
        );
        assert_eq!(
            hex(&hmac_sha512(&[0x0b; 20], b"Hi There")),
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cde\
             daa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        );
        assert_eq!(
            hex(&hmac_sha512(&[0xaa; 20], &[0xdd; 50])),
            "fa73b0089d56a284efb0f0756c890be9b1b5dbdd8ee81a3655f83e33b2279d39\
             bf3e848279a722c806b485a47e67c807b946a337bee8942674278859e13292fb"
        );
    }

    #[test]
    fn codes_match_rfc_6238() {
        // The RFC's 8-digit values, reduced to the last six.
        let key = b"12345678901234567890";
        for (time, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(code_at(key, time, TotpParams::default()), code);
        }
    }

    #[test]
    fn codes_match_rfc_6238_sha256_and_sha512() {
        let key256 = b"12345678901234567890123456789012";
        let key512 = b"1234567890123456789012345678901234567890123456789012345678901234";
        let params256 = TotpParams {
            digits: 8,
            algorithm: TotpAlgorithm::Sha256,
            ..TotpParams::default()
        };
        let params512 = TotpParams {
            digits: 8,
            algorithm: TotpAlgorithm::Sha512,
            ..TotpParams::default()
        };
        for (time, sha256_code, sha512_code) in [
            (59, "46119246", "90693936"),
            (1_111_111_109, "68084774", "25091201"),
            (1_111_111_111, "67062674", "99943326"),
            (1_234_567_890, "91819424", "93441116"),
            (2_000_000_000, "90698825", "38618901"),
            (20_000_000_000, "77737706", "47863826"),
        ] {
            assert_eq!(code_at(key256, time, params256), sha256_code, "{}", time);
            assert_eq!(code_at(key512, time, params512), sha512_code, "{}", time);
        }
    }

    #[test]
    fn period_and_digits_are_honoured() {
        let key = b"12345678901234567890";
        let default = code_at(key, 59, TotpParams::default());
        let long = code_at(
            key,
            59,
            TotpParams {
                period: 60,
                ..TotpParams::default()
            },
        );
        assert_ne!(default, long);
        assert_eq!(default, "287082");
        assert!(long.len() == 6);
        let eight = code_at(
            key,
            59,
            TotpParams {
                digits: 8,
                ..TotpParams::default()
            },
        );
        assert_eq!(eight, "94287082");
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
        for uri in [
            "otpauth://totp/x?secret=GEZDGNBV&digits=8",
            "otpauth://totp/x?secret=GEZDGNBV&period=60",
            "otpauth://totp/x?secret=GEZDGNBV&algorithm=SHA256",
            "otpauth://totp/x?secret=GEZDGNBV&algorithm=SHA512",
        ] {
            assert_eq!(normalize_secret(uri).unwrap(), "GEZDGNBV");
        }
        for bad in ["otpauth://totp/x?issuer=y", "!!!"] {
            assert!(normalize_secret(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn otpauth_uri_parameters_are_parsed() {
        let (secret, params) = parse_otpauth_uri(
            "otpauth://totp/Issuer:user?secret=GEZDGNBV&digits=8&period=60&algorithm=SHA256",
        )
        .unwrap();
        assert_eq!(secret, "GEZDGNBV");
        assert_eq!(
            params,
            TotpParams {
                digits: 8,
                period: 60,
                algorithm: TotpAlgorithm::Sha256,
            }
        );

        let (secret, params) = parse_otpauth_uri(" GEZD GNBV ").unwrap();
        assert_eq!(secret, "GEZDGNBV");
        assert_eq!(params, TotpParams::default());

        let (_, params) =
            parse_otpauth_uri("otpauth://totp/x?secret=GEZDGNBV&algorithm=sha512").unwrap();
        assert_eq!(params.algorithm, TotpAlgorithm::Sha512);
    }

    #[test]
    fn out_of_range_uri_parameters_are_refused() {
        for bad in [
            "otpauth://totp/x?secret=GEZDGNBV&digits=5",
            "otpauth://totp/x?secret=GEZDGNBV&digits=9",
            "otpauth://totp/x?secret=GEZDGNBV&digits=abc",
            "otpauth://totp/x?secret=GEZDGNBV&period=0",
            "otpauth://totp/x?secret=GEZDGNBV&period=301",
            "otpauth://totp/x?secret=GEZDGNBV&algorithm=MD5",
            "otpauth://hotp/x?secret=GEZDGNBV",
        ] {
            assert!(parse_otpauth_uri(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn current_code_uses_the_requested_parameters() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let code = current_code_with(
            secret,
            TotpParams {
                digits: 8,
                algorithm: TotpAlgorithm::Sha256,
                ..TotpParams::default()
            },
        )
        .unwrap();
        assert_eq!(code.len(), 8);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(current_code(secret).unwrap().len(), 6);
        assert!(current_code("not base32!").is_none());
    }
}
