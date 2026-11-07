//! Role gating for staff-only endpoints, against a real local server: owner,
//! co-owner, admin and member fixture accounts plus a signed-out visitor.
//! Covers administrator account lists and the feedback inbox.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Reply, TestServer};
use serde_json::json;

const OWNER: &str = "owner@example.com";
const COOWNER: &str = "coowner@example.com";
const ADMIN: &str = "admin@example.com";
const MEMBER: &str = "member@example.com";
const PASS: &str = "FixtureControlsPassphrase37";

/// POST as a staff account: admin passphrase header plus a same-origin `Origin`.
async fn staff_post(
    s: &TestServer,
    sid: Option<&str>,
    path: &str,
    body: serde_json::Value,
) -> Reply {
    s.call_with(
        sid,
        "POST",
        path,
        Some(body),
        &[("X-Admin-Passphrase", PASS), ("Origin", &s.url)],
    )
    .await
}

/// GET with the admin passphrase header (`with_pass`) or without it.
async fn staff_get(s: &TestServer, sid: Option<&str>, path: &str, with_pass: bool) -> Reply {
    let headers: &[(&str, &str)] = if with_pass {
        &[("X-Admin-Passphrase", PASS)]
    } else {
        &[]
    };
    s.call_with(sid, "GET", path, None, headers).await
}

#[tokio::test(flavor = "multi_thread")]
async fn account_lists_follow_roles() {
    let admins = json!({"owners": [OWNER], "coOwners": [COOWNER], "admins": [ADMIN]}).to_string();
    let s = TestServer::start(&[("admins.json", &admins)], "/api/tester/status", 403).await;
    let owner = s.session_for(OWNER);
    let coowner = s.session_for(COOWNER);
    let admin = s.session_for(ADMIN);
    let member = s.session_for(MEMBER);
    // Staff roles set the admin passphrase / unlock the gate.
    for sid in [&owner, &coowner, &admin] {
        let r = staff_post(
            &s,
            Some(sid),
            "/api/admin/passphrase-status",
            json!({"passphrase": PASS}),
        )
        .await;
        assert_eq!(r.status, 200, "{}", r.text);
    }

    // Account search: staff only, and only with the passphrase header.
    let path = "/api/admin/account-search?q=member";
    assert_eq!(
        staff_get(&s, Some(&admin), path, false).await.status,
        403,
        "passphrase required"
    );
    let found = staff_get(&s, Some(&admin), path, true).await;
    assert_eq!(found.status, 200, "{}", found.text);
    assert_eq!(found.json["accounts"][0]["email"], MEMBER);
    assert!([401, 403].contains(&staff_get(&s, Some(&member), path, true).await.status));
    assert!([401, 403].contains(&staff_get(&s, None, path, true).await.status));

    // The full account list is owner-level only.
    assert_eq!(
        staff_get(&s, Some(&admin), "/api/admin/accounts", true)
            .await
            .status,
        403
    );
    let all = staff_get(&s, Some(&owner), "/api/admin/accounts", true).await;
    assert_eq!(all.status, 200, "{}", all.text);
    assert_eq!(all.json["accounts"].as_array().map(Vec::len), Some(4));
}

/// The feedback inbox (`/moderator/feedback/`) shipped once with its route
/// file never declared in `routes/admin/mod.rs`, so every request 404'd.
/// Moderators and admins read it and mark entries handled; only admins
/// delete; members and visitors are refused.
#[tokio::test(flavor = "multi_thread")]
async fn feedback_inbox_is_routed_and_follows_roles() {
    const MODERATOR: &str = "moderator@example.com";
    let admins = json!({"owners": [OWNER], "admins": [ADMIN]}).to_string();
    let moderators = json!([MODERATOR]).to_string();
    let s = TestServer::start(
        &[("admins.json", &admins), ("moderators.json", &moderators)],
        "/api/tester/status",
        403,
    )
    .await;
    let admin = s.session_for(ADMIN);
    let moderator = s.session_for(MODERATOR);
    let member = s.session_for(MEMBER);
    let r = staff_post(
        &s,
        Some(&admin),
        "/api/admin/passphrase-status",
        json!({"passphrase": PASS}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text);

    let sent = s
        .call_with(
            Some(&member),
            "POST",
            "/api/suggest",
            Some(json!({"type": "bug", "text": "Feedback inbox regression test"})),
            &[("Origin", &s.url)],
        )
        .await;
    assert_eq!(sent.status, 200, "{}", sent.text);

    for status in ["new", "handled", "all"] {
        let path = format!("/api/admin/feedback?status={status}");
        let r = staff_get(&s, Some(&admin), &path, true).await;
        assert_eq!(r.status, 200, "{path}: {}", r.text);
        assert!(r.json["entries"].is_array(), "{path}: {}", r.text);
    }
    let inbox = staff_get(
        &s,
        Some(&moderator),
        "/api/admin/feedback?status=new",
        false,
    )
    .await;
    assert_eq!(
        inbox.status, 200,
        "moderators read without a passphrase: {}",
        inbox.text
    );
    let id = inbox.json["entries"][0]["id"]
        .as_i64()
        .expect("submitted entry is listed");

    let handled = staff_post(
        &s,
        Some(&moderator),
        "/api/admin/feedback/status",
        json!({"id": id, "status": "handled"}),
    )
    .await;
    assert_eq!(handled.status, 200, "{}", handled.text);
    let moved = staff_get(&s, Some(&admin), "/api/admin/feedback?status=handled", true).await;
    assert_eq!(moved.json["entries"][0]["id"].as_i64(), Some(id));

    let denied = staff_post(
        &s,
        Some(&moderator),
        "/api/admin/feedback/delete",
        json!({"id": id}),
    )
    .await;
    assert_eq!(denied.status, 403, "moderators can't delete");
    let deleted = staff_post(
        &s,
        Some(&admin),
        "/api/admin/feedback/delete",
        json!({"id": id}),
    )
    .await;
    assert_eq!(deleted.status, 200, "{}", deleted.text);

    assert!([401, 403].contains(
        &staff_get(&s, Some(&member), "/api/admin/feedback", true)
            .await
            .status
    ));
    assert!([401, 403].contains(
        &staff_get(&s, None, "/api/admin/feedback", true)
            .await
            .status
    ));
}
