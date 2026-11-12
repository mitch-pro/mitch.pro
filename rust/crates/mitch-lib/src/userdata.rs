//! Encrypted storage for per-account saves (`/api/userdata`).
//!
//! Each account has one JSON document at `<root>/<sha256(uid)[..32]>/data.json`.
//! It is stored encrypted with [`KeyRing`] (purpose `userdata`, context the
//! folder name, so a file can't be swapped into another account's folder). Files
//! written before encryption existed are plain JSON; they are still readable and
//! are encrypted by [`sweep`] at startup or by the account's next save.
//!
//! Every read-modify-write must hold [`lock`], including [`sweep`] for each
//! file. Without it a sweep could replace a file with a copy it read before a
//! save landed, losing that save.

use crate::keyring::{KeyRing, OpenError};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// `purpose` passed to the key ring for saves.
pub const PURPOSE: &str = "userdata";

static LOCK: Mutex<()> = Mutex::new(());

/// Serializes access to saves. A poisoned lock is still usable: the guarded
/// data is files, not memory.
pub fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The root folder for saves. `USERDATA_DIR` overrides `<data dir>/userdata`.
pub fn root(data_dir: &Path) -> PathBuf {
    std::env::var_os("USERDATA_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("userdata"))
}

fn context_of(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The stored data exists but can't be decrypted (wrong or missing key, or a
/// damaged file). Callers must not treat this as an empty document: writing
/// over it would destroy the account's saves.
#[derive(Debug)]
pub struct Unreadable;

/// Reads a save document. `Ok(None)` when there is no file or a legacy
/// plaintext file isn't valid JSON (the old behaviour: treat as empty).
pub fn read(ring: &KeyRing, path: &Path) -> Result<Option<Value>, Unreadable> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Unreadable),
    };
    let plain = match ring.open(PURPOSE, &context_of(path), &bytes) {
        Ok(p) => p,
        Err(OpenError::NotEncrypted) => bytes,
        Err(_) => return Err(Unreadable),
    };
    Ok(serde_json::from_slice(&plain).ok())
}

/// Encrypts `json` and replaces the file atomically (temp file, then rename),
/// so a crash can't leave a half-written ciphertext.
pub fn write(ring: &KeyRing, path: &Path, json: &str) -> std::io::Result<()> {
    let sealed = ring.seal(PURPOSE, &context_of(path), json.as_bytes());
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, sealed)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Plaintext files that are now encrypted.
    pub encrypted: u32,
    /// Files re-encrypted from an older key to the primary key.
    pub resealed: u32,
    /// Already under the primary key.
    pub current: u32,
    /// Couldn't be read or rewritten; left untouched.
    pub failed: u32,
}

/// Encrypts every plaintext save under `root` and moves files written under an
/// older key to the primary key. Safe to run on every start.
pub fn sweep(ring: &KeyRing, root: &Path) -> SweepReport {
    let mut report = SweepReport::default();
    let Ok(dirs) = std::fs::read_dir(root) else {
        return report;
    };
    for dir in dirs.flatten() {
        let path = dir.path().join("data.json");
        let _guard = lock();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if ring.is_current(&bytes) {
            report.current += 1;
            continue;
        }
        let was_plain = !KeyRing::is_encrypted(&bytes);
        let plain = match ring.open(PURPOSE, &context_of(&path), &bytes) {
            Ok(p) => p,
            Err(OpenError::NotEncrypted) => bytes,
            Err(_) => {
                report.failed += 1;
                continue;
            }
        };
        match std::str::from_utf8(&plain)
            .map_err(|_| ())
            .and_then(|json| write(ring, &path, json).map_err(|_| ()))
        {
            Ok(()) if was_plain => report.encrypted += 1,
            Ok(()) => report.resealed += 1,
            Err(()) => report.failed += 1,
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn root_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "userdata-test-{}",
            crate::crypto::random_bytes_hex(8)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn written_files_are_ciphertext_and_read_back() {
        let ring = KeyRing::for_tests([3; 32]);
        let root = root_dir();
        let dir = root.join("aaaa");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");
        write(&ring, &path, r#"{"score":42}"#).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("score"));
        assert_eq!(read(&ring, &path).unwrap(), Some(json!({"score": 42})));
        assert!(!dir.join("data.json.tmp").exists());
        // A file moved into another account's folder doesn't open.
        let other = root.join("bbbb");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::copy(&path, other.join("data.json")).unwrap();
        assert!(read(&ring, &other.join("data.json")).is_err());
        // Missing file is empty, not an error.
        assert_eq!(
            read(&ring, &root.join("none").join("data.json")).unwrap(),
            None
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wrong_key_is_unreadable_not_empty() {
        let root = root_dir();
        let path = root.join("cccc").join("data.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write(&KeyRing::for_tests([1; 32]), &path, "{}").unwrap();
        assert!(read(&KeyRing::for_tests([2; 32]), &path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sweep_encrypts_plaintext_and_moves_old_keys() {
        let root = root_dir();
        let plain = root.join("p1");
        let old = root.join("p2");
        let junk = root.join("p3");
        for d in [&plain, &old, &junk] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(plain.join("data.json"), r#"{"a":1}"#).unwrap();
        let old_ring = KeyRing::for_tests([1; 32]);
        write(&old_ring, &old.join("data.json"), r#"{"b":2}"#).unwrap();
        std::fs::write(junk.join("data.json"), [0xff, 0xfe]).unwrap();

        let ring = KeyRing::for_tests([1; 32]);
        let first = sweep(&ring, &root);
        assert_eq!(first.encrypted + first.current, 2);
        assert_eq!(first.failed, 1, "undecodable bytes are left alone");
        assert!(KeyRing::is_encrypted(
            &std::fs::read(plain.join("data.json")).unwrap()
        ));
        assert_eq!(
            read(&ring, &plain.join("data.json")).unwrap(),
            Some(json!({"a": 1}))
        );
        assert_eq!(sweep(&ring, &root).encrypted, 0, "idempotent");
        std::fs::remove_dir_all(root).unwrap();
    }
}
