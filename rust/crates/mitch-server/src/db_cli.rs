//! `mitch-server encrypt-db` and `decrypt-db`: convert `mitchpro.db` between
//! plaintext and SQLCipher (docs/database-encryption.md).
//!
//! Run with every service that uses the database stopped. The conversion writes
//! a new file next to the database and verifies it (same tables, same row
//! counts, passes `quick_check`) before swapping it in; the original is kept
//! under a timestamped name for you to delete once the new one has proven itself.

use mitch_lib::data::DataStore;
use mitch_lib::db_crypto;
use mitch_lib::keyring::KeyRing;
use std::path::{Path, PathBuf};

/// Runs `command` (`encrypt-db` or `decrypt-db`) and returns the process exit code.
pub fn run(command: &str) -> i32 {
    match convert(command) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{command} failed: {message}");
            1
        }
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

fn convert(command: &str) -> Result<(), String> {
    let encrypting = match command {
        "encrypt-db" => true,
        "decrypt-db" => false,
        other => return Err(format!("unknown command {other}")),
    };
    let cfg = crate::hosts::SiteConfig::load();
    let ring = KeyRing::load(&cfg.data_dir).map_err(|e| e.to_string())?;
    println!(
        "Using key {} ({})",
        ring.primary_id(),
        match ring.source() {
            mitch_lib::keyring::KeySource::External => "from the environment or secrets manager",
            mitch_lib::keyring::KeySource::File => "from data/encryption.key",
            mitch_lib::keyring::KeySource::Generated =>
                "NEWLY GENERATED into data/encryption.key: set DATA_ENCRYPTION_KEY to it before starting the services",
        }
    );
    let path = DataStore::db_path(&cfg.data_dir);
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    let staged = sibling(
        &path,
        if encrypting {
            ".encrypted"
        } else {
            ".decrypted"
        },
    );
    let counts = if encrypting {
        db_crypto::encrypt_file(&path, &staged, &ring)
    } else {
        db_crypto::decrypt_file(&path, &staged, &ring)
    }
    .map_err(|e| e.to_string())?;

    let keep = sibling(
        &path,
        &format!(
            ".{}-{}",
            if encrypting { "plaintext" } else { "encrypted" },
            mitch_lib::school::now_millis()
        ),
    );
    std::fs::rename(&path, &keep).map_err(|e| e.to_string())?;
    std::fs::rename(&staged, &path).map_err(|e| e.to_string())?;
    // Empty leftovers of the old file's WAL mode would be misread by the new one.
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(sibling(&path, suffix));
    }

    println!("{} tables converted and verified:", counts.len());
    for (table, rows) in counts {
        println!("  {table}: {rows} rows");
    }
    println!("New database: {}", path.display());
    println!("Original kept as: {}", keep.display());
    if encrypting {
        println!(
            "Start the services with DB_ENCRYPTION=1 and the same key. The original is plaintext; \
             delete it (and any Litestream replica of it) once the encrypted database has been \
             running and snapshotting correctly."
        );
    } else {
        println!("Start the services without DB_ENCRYPTION.");
    }
    Ok(())
}
