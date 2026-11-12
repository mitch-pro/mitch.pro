//! Master encryption key for data at rest (see `docs/encryption-at-rest.md`).
//!
//! One 32-byte master key, supplied by the secrets manager, protects saves
//! (and later the database). It is never stored beside the data it protects
//! once the operator has moved it to the secrets manager.
//!
//! # Where the key comes from
//!
//! In order:
//!
//! 1. `DATA_ENCRYPTION_KEY` in the environment. `doppler run` and
//!    `infisical run` inject it, and `set_external_key` lets the server hand
//!    over one it fetched from the Infisical API itself.
//! 2. `<data dir>/encryption.key`, the bootstrap file.
//! 3. Neither: a new random key is generated and written to that file
//!    (mode 0600). The server then sends a notification, without the key in
//!    it, asking the operator to copy the key into the secrets manager and
//!    delete the file.
//!
//! `DATA_ENCRYPTION_KEY_PREVIOUS` (comma-separated) keeps old keys readable
//! after a rotation. New writes always use the primary key; the startup sweep
//! re-encrypts anything written under another key.
//!
//! # Not silently losing data
//!
//! `<data dir>/encryption.keyid` records the id (a fingerprint, not the key) of
//! the key last used. If that id is no longer in the key ring, because the
//! secrets manager is misconfigured or a rotation dropped the old key too
//! early, `KeyRing::load` fails instead of generating a fresh key that would
//! orphan every encrypted file.
//!
//! # Format
//!
//! `MPENC\x01 | key id (4) | nonce (12) | AES-256-GCM ciphertext + tag`. The
//! cipher key is HKDF-SHA256 of the master key with `purpose` and `context`
//! (for saves, the account's folder name), and both are also authenticated
//! data, so a ciphertext can't be moved to another account's file.

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type HmacSha256 = Hmac<Sha256>;

/// Environment variable holding the master key (64 hex characters or base64
/// of 32 bytes).
pub const KEY_ENV: &str = "DATA_ENCRYPTION_KEY";
/// Comma-separated older keys, kept so existing files stay readable.
pub const PREVIOUS_KEYS_ENV: &str = "DATA_ENCRYPTION_KEY_PREVIOUS";
/// Bootstrap key file in the data dir.
pub const KEY_FILE: &str = "encryption.key";
/// Records the id of the key last used (never the key itself).
pub const KEY_ID_FILE: &str = "encryption.keyid";

const MAGIC: &[u8; 6] = b"MPENC\x01";
const HEADER_LEN: usize = MAGIC.len() + 4 + 12;

static EXTERNAL_KEY: OnceLock<String> = OnceLock::new();

/// Supplies a key fetched from a secrets manager API (Infisical). It is used
/// as if it had been in `DATA_ENCRYPTION_KEY`. Only the first call has an
/// effect, and it must happen before `KeyRing::load`.
pub fn set_external_key(value: String) {
    let _ = EXTERNAL_KEY.set(value);
}

/// Where the primary key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// The environment or a secrets manager. The intended steady state.
    External,
    /// The bootstrap file was already there. Move the key to the secrets
    /// manager and delete the file.
    File,
    /// There was no key, so one was just generated into the bootstrap file.
    Generated,
}

#[derive(Debug)]
pub enum KeyError {
    /// A configured key is not 32 bytes of hex or base64.
    Malformed(&'static str),
    /// Encrypted data exists under a key that isn't available. Refusing to
    /// start is safer than generating a new key and orphaning it.
    MissingKey {
        expected_id: String,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Malformed(which) => write!(
                f,
                "{which} is not a valid key: expected 64 hex characters or base64 of 32 bytes"
            ),
            KeyError::MissingKey { expected_id } => write!(
                f,
                "data was encrypted with key {expected_id}, which is not available: set \
                 {KEY_ENV} (or {PREVIOUS_KEYS_ENV}) to it, or restore {KEY_FILE}"
            ),
            KeyError::Io(e) => write!(f, "key file error: {e}"),
        }
    }
}

impl std::error::Error for KeyError {}

impl From<std::io::Error> for KeyError {
    fn from(e: std::io::Error) -> Self {
        KeyError::Io(e)
    }
}

#[derive(Debug)]
pub enum OpenError {
    /// Not our format (legacy plaintext).
    NotEncrypted,
    /// Encrypted under a key that isn't in the ring.
    UnknownKey,
    /// Wrong key, wrong context, or tampered data.
    Failed,
}

#[derive(Clone)]
struct Key {
    id: [u8; 4],
    bytes: [u8; 32],
}

impl Key {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        let digest = Sha256::digest([b"mitch-key-id:".as_slice(), &bytes].concat());
        Key {
            id: [digest[0], digest[1], digest[2], digest[3]],
            bytes,
        }
    }
}

/// The primary key plus any older keys that can still be read.
pub struct KeyRing {
    primary: Key,
    others: Vec<Key>,
    source: KeySource,
    /// The bootstrap file is still there although the key is now configured
    /// elsewhere, so it should be deleted.
    stale_file: bool,
    file_path: PathBuf,
}

fn parse_key(text: &str, which: &'static str) -> Result<Key, KeyError> {
    let text = text.trim();
    let bytes: Vec<u8> = if text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        (0..32)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16))
            .collect::<Result<_, _>>()
            .map_err(|_| KeyError::Malformed(which))?
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text))
            .map_err(|_| KeyError::Malformed(which))?
    };
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| KeyError::Malformed(which))?;
    Ok(Key::from_bytes(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// HKDF-SHA256 (RFC 5869) with an empty salt, one output block.
fn hkdf32(master: &[u8; 32], info: &[u8]) -> [u8; 32] {
    #[allow(clippy::expect_used)] // HMAC accepts any key length
    let mut extract = <HmacSha256 as Mac>::new_from_slice(&[0u8; 32]).expect("hmac key");
    extract.update(master);
    let prk = extract.finalize().into_bytes();
    #[allow(clippy::expect_used)]
    let mut expand = <HmacSha256 as Mac>::new_from_slice(&prk).expect("hmac key");
    expand.update(info);
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

fn derive(key: &Key, purpose: &str, context: &str) -> (Aes256Gcm, Vec<u8>) {
    let aad = [purpose.as_bytes(), b"\0", context.as_bytes()].concat();
    let sub = hkdf32(
        &key.bytes,
        &[b"mitch-at-rest-v1\0".as_slice(), &aad].concat(),
    );
    // A 32-byte key is always valid for AES-256.
    #[allow(clippy::expect_used)]
    let cipher = Aes256Gcm::new_from_slice(&sub).expect("32-byte key");
    (cipher, aad)
}

impl KeyRing {
    /// Loads or creates the key ring. See the module docs for the order.
    pub fn load(data_dir: &Path) -> Result<Self, KeyError> {
        std::fs::create_dir_all(data_dir)?;
        let file_path = data_dir.join(KEY_FILE);
        let external = EXTERNAL_KEY
            .get()
            .cloned()
            .or_else(|| std::env::var(KEY_ENV).ok())
            .filter(|v| !v.trim().is_empty());
        let external = external.map(|v| parse_key(&v, KEY_ENV)).transpose()?;
        let from_file = match std::fs::read_to_string(&file_path) {
            Ok(text) => Some(parse_key(&text, KEY_FILE)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let mut previous = Vec::new();
        if let Ok(list) = std::env::var(PREVIOUS_KEYS_ENV) {
            for item in list.split(',').filter(|s| !s.trim().is_empty()) {
                previous.push(parse_key(item, PREVIOUS_KEYS_ENV)?);
            }
        }

        let stale_file = external.is_some() && from_file.is_some();
        let (primary, source, mut others) = match (external, from_file) {
            (Some(ext), file) => (ext, KeySource::External, file.into_iter().collect()),
            (None, Some(file)) => (file, KeySource::File, Vec::new()),
            (None, None) => {
                // Nothing configured. If data was encrypted before, a new key
                // would orphan it, so stop instead (checked below).
                if let Some(expected) = read_key_id(data_dir) {
                    if !previous.iter().any(|k| hex(&k.id) == expected) {
                        return Err(KeyError::MissingKey {
                            expected_id: expected,
                        });
                    }
                }
                let mut bytes = [0u8; 32];
                rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
                write_new_key_file(&file_path, &hex(&bytes))?;
                (Key::from_bytes(bytes), KeySource::Generated, Vec::new())
            }
        };
        others.extend(previous);
        others.retain(|k| k.id != primary.id);

        if let Some(expected) = read_key_id(data_dir) {
            let known =
                hex(&primary.id) == expected || others.iter().any(|k| hex(&k.id) == expected);
            if !known {
                return Err(KeyError::MissingKey {
                    expected_id: expected,
                });
            }
        }
        std::fs::write(data_dir.join(KEY_ID_FILE), hex(&primary.id))?;
        Ok(KeyRing {
            primary,
            others,
            source,
            stale_file,
            file_path,
        })
    }

    /// A ring with a fixed key, for tests.
    pub fn for_tests(bytes: [u8; 32]) -> Self {
        KeyRing {
            primary: Key::from_bytes(bytes),
            others: Vec::new(),
            source: KeySource::External,
            stale_file: false,
            file_path: PathBuf::new(),
        }
    }

    pub fn source(&self) -> KeySource {
        self.source
    }

    /// Id of the key new data is written with (a fingerprint; safe to log).
    pub fn primary_id(&self) -> String {
        hex(&self.primary.id)
    }

    pub fn key_file(&self) -> &Path {
        &self.file_path
    }

    /// The message to send the operator while the key is only in the data
    /// dir, as `(title, body)`. Never contains the key.
    pub fn setup_notice(&self) -> Option<(String, String)> {
        let file = self.file_path.display();
        match self.source {
            KeySource::External if self.stale_file => Some((
                "Delete the old encryption key file".into(),
                format!(
                    "The key is now configured in your secrets manager, but {file} is still in \
                     the data folder. Delete it."
                ),
            )),
            KeySource::External => None,
            KeySource::Generated => Some((
                "New data encryption key".into(),
                format!(
                    "A new encryption key was generated in {file}. Copy its contents into your \
                     secrets manager as {KEY_ENV}, then delete the file. Until then the key is \
                     stored beside the data it protects."
                ),
            )),
            KeySource::File => Some((
                "Encryption key still in the data folder".into(),
                format!(
                    "The server is using the key in {file}. Copy it into your secrets manager as \
                     {KEY_ENV}, then delete the file."
                ),
            )),
        }
    }

    /// Whether `data` was produced by [`seal`](Self::seal).
    pub fn is_encrypted(data: &[u8]) -> bool {
        data.starts_with(MAGIC)
    }

    /// Whether `data` is encrypted under the primary key (otherwise it should
    /// be re-sealed).
    pub fn is_current(&self, data: &[u8]) -> bool {
        Self::is_encrypted(data)
            && data.len() >= HEADER_LEN
            && data[MAGIC.len()..MAGIC.len() + 4] == self.primary.id
    }

    /// Encrypts `plaintext` for `purpose` and `context` under the primary key.
    pub fn seal(&self, purpose: &str, context: &str, plaintext: &[u8]) -> Vec<u8> {
        let (cipher, aad) = derive(&self.primary, purpose, context);
        let nonce: [u8; 12] = rand::random();
        let sealed = cipher
            .encrypt(
                GenericArray::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            // AES-GCM encryption only fails past 64 GiB.
            .unwrap_or_default();
        let mut out = Vec::with_capacity(HEADER_LEN + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.primary.id);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        out
    }

    /// Decrypts data from [`seal`](Self::seal) with the same `purpose` and
    /// `context`.
    pub fn open(&self, purpose: &str, context: &str, data: &[u8]) -> Result<Vec<u8>, OpenError> {
        if !Self::is_encrypted(data) {
            return Err(OpenError::NotEncrypted);
        }
        if data.len() < HEADER_LEN + 16 {
            return Err(OpenError::Failed);
        }
        let id = &data[MAGIC.len()..MAGIC.len() + 4];
        let key = std::iter::once(&self.primary)
            .chain(self.others.iter())
            .find(|k| k.id == id)
            .ok_or(OpenError::UnknownKey)?;
        let (cipher, aad) = derive(key, purpose, context);
        let nonce = &data[MAGIC.len() + 4..HEADER_LEN];
        cipher
            .decrypt(
                GenericArray::from_slice(nonce),
                Payload {
                    msg: &data[HEADER_LEN..],
                    aad: &aad,
                },
            )
            .map_err(|_| OpenError::Failed)
    }
}

fn read_key_id(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(data_dir.join(KEY_ID_FILE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_new_key_file(path: &Path, hex_key: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(hex_key.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "keyring-test-{}",
            crate::crypto::random_bytes_hex(8)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn seal_round_trips_and_binds_context() {
        let ring = KeyRing::for_tests([7; 32]);
        let sealed = ring.seal("userdata", "abc", b"hello");
        assert!(KeyRing::is_encrypted(&sealed));
        assert!(!sealed.windows(5).any(|w| w == b"hello"));
        assert_eq!(ring.open("userdata", "abc", &sealed).unwrap(), b"hello");
        assert!(matches!(
            ring.open("userdata", "other", &sealed),
            Err(OpenError::Failed)
        ));
        assert!(matches!(
            ring.open("db", "abc", &sealed),
            Err(OpenError::Failed)
        ));
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(matches!(
            ring.open("userdata", "abc", &tampered),
            Err(OpenError::Failed)
        ));
        assert!(matches!(
            ring.open("userdata", "abc", b"{\"plain\":1}"),
            Err(OpenError::NotEncrypted)
        ));
    }

    #[test]
    fn another_key_cannot_open_it() {
        let a = KeyRing::for_tests([1; 32]);
        let b = KeyRing::for_tests([2; 32]);
        let sealed = a.seal("userdata", "x", b"data");
        assert!(matches!(
            b.open("userdata", "x", &sealed),
            Err(OpenError::UnknownKey)
        ));
        assert!(a.is_current(&sealed) && !b.is_current(&sealed));
    }

    #[test]
    fn parses_hex_and_base64() {
        let hex_key = "ab".repeat(32);
        let b64 = base64::engine::general_purpose::STANDARD.encode([0xab; 32]);
        assert_eq!(
            parse_key(&hex_key, "t").unwrap().id,
            parse_key(&b64, "t").unwrap().id
        );
        assert!(parse_key("short", "t").is_err());
        assert!(parse_key(&"zz".repeat(32), "t").is_err());
    }

    /// One test covers the load paths, because they read process-wide
    /// environment variables that parallel tests would race on.
    #[test]
    fn load_generates_then_reuses_and_refuses_orphaning() {
        std::env::remove_var(KEY_ENV);
        std::env::remove_var(PREVIOUS_KEYS_ENV);
        let d = dir();

        let first = KeyRing::load(&d).unwrap();
        assert_eq!(first.source(), KeySource::Generated);
        let notice = first.setup_notice().unwrap().1;
        let key_text = std::fs::read_to_string(d.join(KEY_FILE)).unwrap();
        assert_eq!(key_text.len(), 64);
        assert!(
            !notice.contains(&key_text),
            "notice must not contain the key"
        );
        let sealed = first.seal("userdata", "u", b"saved");

        let again = KeyRing::load(&d).unwrap();
        assert_eq!(again.source(), KeySource::File);
        assert_eq!(again.open("userdata", "u", &sealed).unwrap(), b"saved");

        // The file is gone and no key was configured: refuse, don't regenerate.
        std::fs::remove_file(d.join(KEY_FILE)).unwrap();
        match KeyRing::load(&d) {
            Err(KeyError::MissingKey { expected_id }) => {
                assert_eq!(expected_id, first.primary_id())
            }
            other => panic!("expected MissingKey, got {:?}", other.err()),
        }

        // The key moved to the environment: loads, quietly.
        std::env::set_var(KEY_ENV, &key_text);
        let moved = KeyRing::load(&d).unwrap();
        assert_eq!(moved.source(), KeySource::External);
        assert!(moved.setup_notice().is_none());
        // With the file back beside a configured key, the operator is asked to delete it.
        std::fs::write(d.join(KEY_FILE), &key_text).unwrap();
        let stale = KeyRing::load(&d).unwrap();
        assert!(stale.setup_notice().unwrap().1.contains("Delete it"));
        std::fs::remove_file(d.join(KEY_FILE)).unwrap();
        assert_eq!(moved.open("userdata", "u", &sealed).unwrap(), b"saved");

        // A different key without the old one as a previous key is refused.
        std::env::set_var(KEY_ENV, "cd".repeat(32));
        assert!(matches!(
            KeyRing::load(&d),
            Err(KeyError::MissingKey { .. })
        ));
        std::env::set_var(PREVIOUS_KEYS_ENV, &key_text);
        let rotated = KeyRing::load(&d).unwrap();
        assert_ne!(rotated.primary_id(), first.primary_id());
        assert_eq!(rotated.open("userdata", "u", &sealed).unwrap(), b"saved");
        assert!(!rotated.is_current(&sealed));

        std::env::set_var(KEY_ENV, "not a key");
        assert!(matches!(KeyRing::load(&d), Err(KeyError::Malformed(_))));
        std::env::remove_var(KEY_ENV);
        std::env::remove_var(PREVIOUS_KEYS_ENV);
        std::fs::remove_dir_all(d).unwrap();
    }
}
