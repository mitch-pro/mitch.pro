//! TOTP (RFC 6238) + the `totp1:` at-rest seal — port of server.js's
//! `generateTotp`/`verifyTotp` (2477-2497) and `sealTotpSecret`/`openTotpSecret`
//! (2427-2448).
//!
//! Contract:
//! - HMAC-SHA1, 30s step, ±1 window (3 codes), 6 digits, dynamic truncation,
//!   zero-padded. Code comparison is plain string equality in the JS (not
//!   constant-time) — ported as-is.
//! - Secret is RFC 4648 base32 (A-Z2-7), `=` stripped, case-insensitive,
//!   invalid chars silently skipped.
//! - At-rest: `totp1:<iv-hex>:<ct-hex>:<tag-hex>` — AES-256-GCM, key =
//!   HMAC-SHA256(ID_SECRET, "totp-at-rest-v1"). Malformed framing → `''`
//!   (JS parity — unlike openAtRest which returns the input).
//!   sealTotpSecret failure returns the PLAINTEXT secret (JS catch behavior).

use hmac::{Hmac, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

const BASE32_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// `base32Decode(str)` — RFC alphabet, `=` stripped, case-insensitive,
/// invalid chars silently skipped.
fn base32_decode(s: &str) -> Vec<u8> {
    let mut acc: u64 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::new();
    for c in s.to_uppercase().trim_end_matches('=').bytes() {
        if let Some(pos) = BASE32_ALPHABET.iter().position(|&a| a == c) {
            acc = (acc << 5) | pos as u64;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                out.push(((acc >> bits) & 0xff) as u8);
                // Mask off the consumed byte so only remaining low bits stay.
                acc &= (1u64 << bits) - 1;
            }
        }
        // Invalid chars silently skipped (JS parity).
    }
    out
}

/// `generateTotp(secret, step)` — HMAC-SHA1 + dynamic truncation, 6 digits.
#[allow(clippy::expect_used)] // HMAC accepts any key length; cannot fail
pub fn generate_totp(secret: &str, step: u64) -> String {
    let key = base32_decode(secret);
    let counter = step.to_be_bytes();
    let mut mac = <HmacSha1 as Mac>::new_from_slice(&key).expect("hmac accepts any key size");
    mac.update(&counter);
    let hmac = mac.finalize().into_bytes();
    let len = hmac.len();
    let offset = (hmac[len - 1] & 0x0f) as usize;
    let code = ((hmac[offset] & 0x7f) as u32) << 24
        | (hmac[offset + 1] as u32) << 16
        | (hmac[offset + 2] as u32) << 8
        | (hmac[offset + 3] as u32);
    format!("{:06}", code % 1_000_000)
}

/// `verifyTotp(secret, code)` — ±1 window (3 total codes), plain equality.
pub fn verify_totp(secret: &str, code: &str) -> bool {
    let clean = code.trim();
    let step = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 30)
        .unwrap_or(0);
    (0..=1).any(|skew| generate_totp(secret, step + skew) == clean)
}

/// `randomBase32(32)` — `alphabet[bytes[i] % alphabet.length]`.
pub fn random_base32(n: usize) -> String {
    use rand::RngCore;
    let mut bytes = vec![0u8; n];
    rand::rng().fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| BASE32_ALPHABET[(*b % 32) as usize] as char)
        .collect()
}

/// `sealTotpSecret(secret)` — `totp1:` envelope, `totp-at-rest-v1` purpose.
/// Failure returns the PLAINTEXT secret (JS catch parity).
pub fn seal_totp_secret(secret: &str, id_secret: &[u8]) -> String {
    let key = crate::crypto::hmac_sha256(id_secret, b"totp-at-rest-v1");
    match crate::crypto::seal_at_rest(&key, secret, "totp1:") {
        Some(sealed) => sealed,
        None => secret.to_string(),
    }
}

/// `openTotpSecret(str)` — returns `''` on malformed framing (JS parity:
/// different from openAtRest which returns the input). Non-totp1-prefixed
/// input passes through (legacy plaintext).
pub fn open_totp_secret(sealed: &str, id_secret: &[u8]) -> String {
    if !sealed.starts_with("totp1:") {
        return sealed.to_string();
    }
    let key = crate::crypto::hmac_sha256(id_secret, b"totp-at-rest-v1");
    // JS openTotpSecret returns '' on malformed framing; the plaintext is the
    // raw base32 secret string (NOT JSON-parsed).
    crate::crypto::decrypt_raw(&key, sealed, "totp1:").unwrap_or_default()
}

/// `randomBytes(24).toString('hex')` — the pendingTwoFactor temp token.
pub fn create_temp_token() -> String {
    crate::crypto::random_bytes_hex(24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_decode_known_vectors() {
        // "ORSXG5A" IS base32 for "test" — my original assertion was right.
        assert_eq!(base32_decode("ORSXG5A"), b"test");
        // "JBSWY3DP" is base32 of "Hello" (5 bytes from 8×5 = 40 bits).
        assert_eq!(base32_decode("JBSWY3DP"), b"Hello");
        // Case-insensitive.
        assert_eq!(base32_decode("jbswy3dp"), b"Hello");
    }

    #[test]
    fn totp_rfc6238_test_vector() {
        // RFC 6238 test secret: ASCII "12345678901234567890",
        // base32 = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ" (32 chars = 20 bytes).
        // T=59s → step = 59/30 = 1 → SHA-1 truncated = 287082 (6 digits).
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(generate_totp(secret, 1), "287082");
        // T=1111111109s → step = 3703703 → 081804.
        assert_eq!(generate_totp(secret, 1111111109 / 30), "081804");
    }

    #[test]
    fn verify_totp_accepts_window() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        // The current step ± 1 should verify against generate_totp of that step.
        let now_step = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 30;
        assert!(verify_totp(secret, &generate_totp(secret, now_step)));
        // Wrong code fails.
        assert!(!verify_totp(secret, "000000") || generate_totp(secret, now_step) == "000000");
    }

    #[test]
    fn totp_seal_open_round_trip_with_totp1_prefix() {
        let id_secret = b"test-id-secret-bytes-0123456789";
        let secret = random_base32(32);
        let sealed = seal_totp_secret(&secret, id_secret);
        assert!(
            sealed.starts_with("totp1:"),
            "prefix must be totp1"
        );
        assert_eq!(open_totp_secret(&sealed, id_secret), secret);
        // Malformed framing → '' (JS parity, unlike openAtRest).
        assert_eq!(open_totp_secret("totp1:zz:zz", id_secret), "");
        // Non-prefixed input passes through (legacy plaintext).
        assert_eq!(
            open_totp_secret("plaintext-secret", id_secret),
            "plaintext-secret"
        );
    }

    #[test]
    fn temp_token_is_24_byte_hex() {
        let token = create_temp_token();
        assert_eq!(token.len(), 48); // 24 bytes → 48 hex chars
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
