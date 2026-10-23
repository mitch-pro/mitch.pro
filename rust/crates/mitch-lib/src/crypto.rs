#![allow(deprecated)]

//! At-rest crypto + ID helpers — byte-faithful port of server.js's
//! `sealAtRest`/`openAtRest` (dm + totp purposes), the `makeEmailId` HMAC
//! scheme, and the ID_SECRET bootstrap.
//!
//! Contract:
//! - key = HMAC-SHA256(key = ID_SECRET, data = <purpose>).digest() — 32 bytes.
//!   Purposes: `dm-at-rest-v1`, `totp-at-rest-v1`.
//! - sealed format: `enc1:<iv-hex>:<ct-hex>:<tag-hex>` — AES-256-GCM,
//!   12-byte random IV, ct = AES ciphertext (tag split off by the JS), tag
//!   = 16-byte GCM tag.
//! - `openAtRest` returns the original string on any failure (parity), and
//!   the parsed value when it is an object.
//! - ID_SECRET: read from `data/id_secret.key` as raw bytes; on missing,
//!   32 random bytes + persisted back (JS readFileSync/randomBytes parity).

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::data::DataError;

type HmacSha256 = Hmac<Sha256>;

pub const DM_AT_REST_PREFIX: &str = "enc1:";
pub const TOTP_AT_REST_PREFIX: &str = "totp1:";
pub const DM_AT_REST_PURPOSE: &str = "dm-at-rest-v1";
pub const TOTP_AT_REST_PURPOSE: &str = "totp-at-rest-v1";

/// ID_SECRET — raw bytes from `data/id_secret.key`; generated once when
/// missing (JS readFileSync/randomBytes+writeFileSync parity).
pub fn load_id_secret(data_dir: &Path) -> Result<Vec<u8>, DataError> {
    let file = data_dir.join("id_secret.key");
    if let Ok(bytes) = std::fs::read(&file) {
        return Ok(bytes);
    }
    let mut secret = vec![0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut secret);
    std::fs::write(&file, &secret)?;
    Ok(secret)
}

/// `HMAC-SHA256(key, data).digest()` — 32 raw bytes.
#[allow(clippy::expect_used)] // new_from_slice only errors on empty keys
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("hmac accepts any key size");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Hex of `HMAC-SHA256(key, data)` (used by the email-id scheme).
pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    hex(&hmac_sha256(key, data))
}

/// `sha256(data).hex` helper (used by email-id + bg-dir derivations).
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let mut out = String::with_capacity(64);
    for b in hasher.finalize() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for b in data.iter() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// AES-256-GCM seal, JS shape: `enc1:<iv-hex>:<ct-hex>:<tag-hex>` (the
/// aes-gcm crate appends the 16-byte tag to the ciphertext, so split it).
/// Returns None when the plaintext is not valid UTF-8 (JS catch -> null).
pub fn seal_at_rest(key: &[u8; 32], json_string: &str, prefix: &str) -> Option<String> {
    let iv: [u8; 12] = rand::random();
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = GenericArray::from_slice(&iv);
    let sealed = cipher.encrypt(nonce, json_string.as_bytes()).ok()?;
    let (ct, tag) = sealed.split_at(sealed.len() - 16);
    Some(format!("{prefix}{}:{}:{}", hex(&iv), hex(ct), hex(tag)))
}

/// AES-256-GCM open, JS shape: parse `enc1:<iv-hex>:<ct-hex>:<tag-hex>`,
/// decrypt, JSON.parse — returning the original string on any failure
/// (parity with the JS catch -> return str).
pub fn open_at_rest(key: &[u8; 32], sealed: &str, prefix: &str) -> Value {
    if !sealed.starts_with(prefix) {
        return Value::String(sealed.to_owned());
    }
    let opened = (|| -> Option<Value> {
        let body = &sealed[prefix.len()..];
        let i1 = body.find(':')?;
        let i2 = body[i1 + 1..].find(':')? + i1 + 1;
        let iv = unhex(&body[..i1])?;
        let ct = unhex(&body[i1 + 1..i2])?;
        let tag = unhex(&body[i2 + 1..])?;
        let cipher = Aes256Gcm::new_from_slice(key).ok()?;
        let nonce = GenericArray::from_slice(&iv);
        let mut combined = ct.clone();
        combined.extend_from_slice(&tag);
        let plain = cipher.decrypt(nonce, combined.as_ref()).ok()?;
        serde_json::from_str::<Value>(std::str::from_utf8(&plain).ok()?).ok()
    })();
    opened.unwrap_or_else(|| Value::String(sealed.to_owned()))
}

/// `randomBytes(n)` — n random bytes.
pub fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut out = vec![0u8; n];
    rand::rng().fill_bytes(&mut out);
    out
}

/// `Math.random()` — a uniform in [0, 1) from 53 random bits (the largest
/// exactly-representable denominator; matches V8's range even though V8
/// derives its value differently).
pub fn js_random() -> f64 {
    let buf: [u8; 8] = random_bytes(8).try_into().unwrap_or([0; 8]);
    let n = u64::from_le_bytes(buf) >> 11;
    n as f64 / (1u64 << 53) as f64
}

/// `arr[Math.floor(Math.random() * arr.length)]`.
pub fn js_random_index(len: usize) -> usize {
    (js_random() * len as f64).floor() as usize
}

/// `genServerKeypair()` (server.js:3993) — an ECDH P-256 pair whose public
/// half exports as the raw 65-byte `04…` point hex. The JS draws the key via
/// WebCrypto; here the 32-byte scalar comes from `random_bytes` with a retry
/// on the (probability-zero) invalid-scalar case, which keeps p256 free of a
/// second rand_core version.
pub fn generate_p256_keypair() -> (p256::SecretKey, String) {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    loop {
        if let Ok(sk) = p256::SecretKey::from_slice(&random_bytes(32)) {
            let pub_hex = hex(sk.public_key().to_encoded_point(false).as_bytes());
            return (sk, pub_hex);
        }
    }
}

/// Standard base64 decode (with padding), mirroring `Buffer.from(s, 'base64')`
/// for the charset-validated inputs the callers pass.
pub fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .unwrap_or_default()
}

/// `randomBytes(n).toString('hex')`.
pub fn random_bytes_hex(n: usize) -> String {
    use rand::RngCore;
    let mut out = vec![0u8; n];
    rand::rng().fill_bytes(&mut out);
    let mut hex = String::with_capacity(out.len() * 2);
    for b in out {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Raw AES-256-GCM decrypt — returns the plaintext string without JSON
/// parsing (for `openTotpSecret` which returns the raw base32 secret, not a
/// JSON value).
pub fn decrypt_raw(key: &[u8; 32], sealed: &str, prefix: &str) -> Option<String> {
    let body = &sealed[prefix.len()..];
    let i1 = body.find(':')?;
    let i2 = body[i1 + 1..].find(':')? + i1 + 1;
    let iv = unhex(&body[..i1])?;
    let ct = unhex(&body[i1 + 1..i2])?;
    let tag = unhex(&body[i2 + 1..])?;
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = GenericArray::from_slice(&iv);
    let mut combined = ct.clone();
    combined.extend_from_slice(&tag);
    let plain = cipher.decrypt(nonce, combined.as_ref()).ok()?;
    String::from_utf8(plain).ok()
}

/// `timingSafeEqual(a, b)` — constant-time compare; false on length mismatch
/// (which JS throws on, so call sites guard lengths first).
pub fn timing_safe_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// `argon2id` verification against Bun.password.hash PHC strings. The params
/// are embedded in the PHC string, so verification matches whatever params
/// bun used.
pub fn argon2_verify(phc_hash: &str, password: &str) -> bool {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    match PasswordHash::new(phc_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// `Bun.password.hash(pw)` — argon2id PHC string (crate defaults).
pub fn argon2_hash(password: &str) -> String {
    use argon2::password_hash::SaltString;
    use argon2::{Argon2, PasswordHasher};
    let Ok(salt) = SaltString::encode_b64(&random_bytes(16)) else {
        return String::new();
    };
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .unwrap_or_default()
}

/// `Disconnect` reason codes for russh teardown parity.
pub const DISCONNECT_BY_APPLICATION: &str = "ByApplication";

const BASE36: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// `Date.now().toString(36)` — lowercase base36 representation of an unsigned integer.
pub fn to_base36(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut buf = [0u8; 13]; // 2^64 < 36^13
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = BASE36[(n % 36) as usize];
        n /= 36;
    }
    String::from_utf8_lossy(&buf[i..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_key() -> [u8; 32] {
        hmac_sha256(b"mitch-e2e", b"crypto-test")
    }

    #[test]
    fn seal_open_round_trips() {
        let key = test_key();
        let obj = serde_json::json!({"text": "secret dm ✅", "n": 42});
        let sealed = seal_at_rest(
            &key,
            &serde_json::to_string(&obj).expect("stringify"),
            DM_AT_REST_PREFIX,
        )
        .expect("seal");
        assert!(sealed.starts_with("enc1:"));
        // iv:ct:tag all hex.
        let parts: Vec<&str> = sealed[5..].split(':').collect();
        assert_eq!(parts.len(), 3);
        for p in &parts {
            assert!(!p.is_empty() && p.chars().all(|c| c.is_ascii_hexdigit()));
        }
        // Round trip.
        let opened = open_at_rest(&key, &sealed, DM_AT_REST_PREFIX);
        assert_eq!(opened, obj);
    }

    #[test]
    fn open_returns_original_on_garbage() {
        let key = test_key();
        assert_eq!(
            open_at_rest(&key, "not-sealed", DM_AT_REST_PREFIX),
            serde_json::json!("not-sealed")
        );
        assert_eq!(
            open_at_rest(&key, "enc1:zz:zz:zz", DM_AT_REST_PREFIX),
            serde_json::json!("enc1:zz:zz:zz")
        );
    }

    #[test]
    fn hmac_matches_known_vector() {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?"
        let expected = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            expected
        );
    }

    #[test]
    fn sha256_matches_known() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn id_secret_persists_once() {
        let dir = std::env::temp_dir().join(format!(
            "mitch-lib-crypto-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let secret1 = load_id_secret(&dir).unwrap();
        let secret2 = load_id_secret(&dir).unwrap();
        assert_eq!(secret1, secret2, "second load must reuse the file");
        assert_eq!(secret1.len(), 32);
        assert!(dir.join("id_secret.key").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
