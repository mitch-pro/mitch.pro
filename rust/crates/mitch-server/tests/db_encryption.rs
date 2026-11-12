//! Database encryption (docs/database-encryption.md): the `encrypt-db` and
//! `decrypt-db` commands convert a real database file and keep the original, and
//! a server started with `DB_ENCRYPTION=1` runs on an encrypted database and
//! writes encrypted snapshots.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;
use std::path::Path;
use std::process::Command;

const KEY: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

fn is_plaintext(path: &Path) -> bool {
    std::fs::read(path)
        .map(|b| b.starts_with(b"SQLite format 3\0"))
        .unwrap_or(false)
}

fn run(dir: &Path, command: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mitch-server"))
        .arg(command)
        .current_dir(dir)
        .env("DATA_DIR", dir.join("data"))
        .env("MITCH_BASE", dir)
        .env("DATA_ENCRYPTION_KEY", KEY)
        .output()
        .expect("run mitch-server")
}

#[test]
fn encrypt_and_decrypt_commands_convert_and_keep_the_original() {
    let dir = std::env::temp_dir().join(format!("mitch-dbcli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("data")).unwrap();
    let db = dir.join("data/mitchpro.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE people(email TEXT); INSERT INTO people VALUES('needle@example.com');",
        )
        .unwrap();
    }

    let out = run(&dir, "encrypt-db");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!is_plaintext(&db), "database is still plaintext");
    assert!(!std::fs::read(&db)
        .unwrap()
        .windows(6)
        .any(|w| w == b"needle"));
    let kept: Vec<_> = std::fs::read_dir(dir.join("data"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".plaintext-"))
        .collect();
    assert_eq!(kept.len(), 1, "the plaintext original is kept");
    assert!(is_plaintext(&kept[0].path()));

    // Running it again on an encrypted database is refused, not repeated.
    assert!(!run(&dir, "encrypt-db").status.success());

    let out = run(&dir, "decrypt-db");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(is_plaintext(&db));
    let conn = rusqlite::Connection::open(&db).unwrap();
    let email: String = conn
        .query_row("SELECT email FROM people", [], |r| r.get(0))
        .unwrap();
    assert_eq!(email, "needle@example.com");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn server_runs_on_an_encrypted_database_and_snapshots_it() {
    let server = TestServer::start_with_env(
        &[],
        "/api/tester/status",
        403,
        &[("DB_ENCRYPTION", "1"), ("DATA_ENCRYPTION_KEY", KEY)],
    )
    .await;
    let db = server.dir.join("data/mitchpro.db");
    assert!(!is_plaintext(&db), "a new database starts encrypted");

    // The first snapshot is written at start.
    let snaps = server.dir.join("data/db-snapshots");
    let mut found = false;
    for _ in 0..50 {
        if std::fs::read_dir(&snaps)
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().ends_with(".db"))
            })
            .unwrap_or(false)
        {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(found, "no snapshot was written");
    for entry in std::fs::read_dir(&snaps).unwrap().flatten() {
        assert!(!is_plaintext(&entry.path()), "snapshots are encrypted");
    }
}
