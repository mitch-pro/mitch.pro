//! VM access consent against a real local server (docs/vm-access.md): an
//! admin can't open a member's VM until the member allows it, only the member
//! can allow it, and turning it off locks the admin out again. The site owner
//! keeps access throughout. Another member never gets in.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Reply, TestServer};
use serde_json::json;

const OWNER: &str = "owner@example.com";
const ADMIN: &str = "admin@example.com";
const MEMBER: &str = "member@example.com";
const OTHER: &str = "other@example.com";
const VM_ID: &str = "vm-fixture";

async fn post(s: &TestServer, sid: &str, path: &str, body: serde_json::Value) -> Reply {
    let origin = s.url.clone();
    s.call_with(
        Some(sid),
        "POST",
        path,
        Some(body),
        &[("Origin", origin.as_str())],
    )
    .await
}

/// 403 means the access check refused; anything else got past it (the VM
/// itself doesn't exist in Proxmox here, so later steps may still fail).
async fn can_open(s: &TestServer, sid: &str) -> bool {
    s.call(
        Some(sid),
        "GET",
        &format!("/api/vm/computers/{VM_ID}"),
        None,
    )
    .await
    .status
        != 403
}

#[tokio::test(flavor = "multi_thread")]
async fn admins_need_the_owners_explicit_consent() {
    let admins = json!({ "owners": [OWNER], "admins": [ADMIN] }).to_string();
    let s = TestServer::start(&[("admins.json", &admins)], "/api/tester/status", 403).await;
    let owner = s.session_for(OWNER);
    let admin = s.session_for(ADMIN);
    let member = s.session_for(MEMBER);
    let other = s.session_for(OTHER);
    {
        let db = rusqlite::Connection::open(s.dir.join("data/mitchpro.db")).unwrap();
        db.execute(
            "INSERT INTO virtual_machines (id, owner_email, proxmox_vmid, proxmox_node, friendly_name,
                hostname, operating_system, status, created_at, updated_at)
             VALUES (?1, ?2, 4242, 'node', 'My Computer', 'host', 'Linux', 'assigned', 1, 1)",
            rusqlite::params![VM_ID, MEMBER],
        )
        .unwrap();
    }
    let access = format!("/api/vm/computers/{VM_ID}/admin-access");

    assert!(can_open(&s, &member).await, "the VM's owner");
    assert!(can_open(&s, &owner).await, "the site owner");
    assert!(!can_open(&s, &admin).await, "admin without consent");
    assert!(!can_open(&s, &other).await, "another member");

    // Only the VM's owner can allow it — not the admin, not the site owner.
    let r = post(&s, &admin, &access, json!({ "allow": true })).await;
    assert_eq!(r.status, 403, "{}", r.text);
    let r = post(&s, &owner, &access, json!({ "allow": true })).await;
    assert_eq!(r.status, 403, "{}", r.text);
    assert!(!can_open(&s, &admin).await);

    let r = post(&s, &member, &access, json!({ "allow": true })).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.json["adminAccess"]["allowed"], true);
    assert!(r.json["adminAccess"]["expiresAt"].as_i64().unwrap() > 0);
    assert!(can_open(&s, &admin).await, "admin with consent");
    assert!(!can_open(&s, &other).await, "consent is for admins only");

    let r = post(&s, &member, &access, json!({ "allow": false })).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.json["adminAccess"]["allowed"], false);
    assert!(!can_open(&s, &admin).await, "admin after consent withdrawn");
    assert!(can_open(&s, &owner).await, "site owner keeps access");
}
