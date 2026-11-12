//! Saves are encrypted on disk (docs/encryption-at-rest.md): what a signed-in
//! member syncs through `/api/userdata` round-trips through the API, but the
//! file on disk is ciphertext, and a key is generated and left for the
//! operator to move to the secrets manager.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn saves_are_ciphertext_on_disk() {
    let server = TestServer::start(&[], "/api/tester/status", 403).await;
    let sid = server.session_for("saver@example.com");

    let saved = server
        .call(
            Some(&sid),
            "POST",
            "/api/userdata",
            Some(json!({ "_snapshot": { "score": "needle-9f3c" } })),
        )
        .await;
    assert_eq!(saved.status, 200, "{}", saved.text);

    let back = server.call(Some(&sid), "GET", "/api/userdata", None).await;
    assert_eq!(back.status, 200);
    assert_eq!(back.json["_snapshot"]["score"], "needle-9f3c");

    let files: Vec<_> = std::fs::read_dir(server.dir.join("userdata"))
        .unwrap()
        .flatten()
        .map(|d| d.path().join("data.json"))
        .collect();
    assert_eq!(files.len(), 1);
    let raw = std::fs::read(&files[0]).unwrap();
    assert!(
        raw.starts_with(b"MPENC"),
        "file is not in the encrypted format"
    );
    assert!(
        !String::from_utf8_lossy(&raw).contains("needle-9f3c"),
        "plaintext found in the saved file"
    );

    // No key was configured, so one was generated for the operator to move.
    let key = std::fs::read_to_string(server.dir.join("data/encryption.key")).unwrap();
    assert_eq!(key.trim().len(), 64);

    // Deleting keys works on the encrypted file too.
    let removed = server
        .call(
            Some(&sid),
            "DELETE",
            "/api/userdata",
            Some(json!({ "keys": ["_snapshot"] })),
        )
        .await;
    assert_eq!(removed.status, 200, "{}", removed.text);
    let after = server.call(Some(&sid), "GET", "/api/userdata", None).await;
    assert!(after.json.get("_snapshot").is_none());
}
