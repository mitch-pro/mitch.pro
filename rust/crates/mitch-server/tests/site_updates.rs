//! The homepage update log against a real local server: admins can post, list,
//! edit and delete entries; everyone else is refused; the public list shows
//! what was posted.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Reply, TestServer};
use serde_json::json;

const ADMIN: &str = "admin@example.com";
const MEMBER: &str = "member@example.com";
const PASS: &str = "FixtureControlsPassphrase37";

async fn admin(
    s: &TestServer,
    sid: Option<&str>,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Reply {
    s.call_with(
        sid,
        method,
        path,
        body,
        &[
            ("X-Admin-Passphrase", PASS),
            ("Origin", &s.url),
            ("X-Mitch-Requested-With", "1"),
        ],
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn admins_manage_the_update_log_and_the_public_sees_it() {
    let admins = json!({"owners": [], "coOwners": [], "admins": [ADMIN]}).to_string();
    let s = TestServer::start(&[("admins.json", &admins)], "/api/tester/status", 403).await;
    let sid = s.session_for(ADMIN);
    let member = s.session_for(MEMBER);

    // Set the admin passphrase so the gate opens.
    let r = admin(
        &s,
        Some(&sid),
        "POST",
        "/api/admin/passphrase-status",
        Some(json!({"passphrase": PASS})),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text);

    // Empty at first, for the public and for the admin.
    let public = s.call(None, "GET", "/api/updates", None).await;
    assert_eq!(public.status, 200, "{}", public.text);
    assert_eq!(public.json["updates"].as_array().unwrap().len(), 0);
    let list = admin(&s, Some(&sid), "GET", "/api/admin/updates", None).await;
    assert_eq!(list.status, 200, "{}", list.text);

    // Post, with and without a date.
    let posted = admin(
        &s,
        Some(&sid),
        "POST",
        "/api/admin/updates",
        Some(json!({"title": "First", "body": "hello", "date": "2026-11-01"})),
    )
    .await;
    assert_eq!(posted.status, 200, "{}", posted.text);
    let id = posted.json["id"].as_i64().unwrap();
    let second = admin(
        &s,
        Some(&sid),
        "POST",
        "/api/admin/updates",
        Some(json!({"title": "Second"})),
    )
    .await;
    assert_eq!(second.status, 200, "{}", second.text);

    // Bad input is refused with a message.
    for body in [
        json!({"title": "  "}),
        json!({"title": "x", "date": "11/01/2026"}),
    ] {
        let r = admin(&s, Some(&sid), "POST", "/api/admin/updates", Some(body)).await;
        assert_eq!(r.status, 400, "{}", r.text);
        assert!(r.json["error"].as_str().is_some());
    }

    // The public list is newest first and carries no author.
    let public = s.call(None, "GET", "/api/updates", None).await;
    let items = public.json["updates"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["title"], "Second");
    assert_eq!(items[1]["title"], "First");
    assert!(items[0].get("author").is_none());
    // The admin list adds the author.
    let list = admin(&s, Some(&sid), "GET", "/api/admin/updates", None).await;
    assert_eq!(list.json["updates"][0]["author"], ADMIN);

    // Edit, then delete.
    let edited = admin(
        &s,
        Some(&sid),
        "POST",
        "/api/admin/updates",
        Some(json!({"id": id, "title": "First, edited", "body": "", "date": "2026-11-01"})),
    )
    .await;
    assert_eq!(edited.status, 200, "{}", edited.text);
    let removed = admin(
        &s,
        Some(&sid),
        "POST",
        "/api/admin/updates/delete",
        Some(json!({"id": id})),
    )
    .await;
    assert_eq!(removed.status, 200, "{}", removed.text);
    let again = admin(
        &s,
        Some(&sid),
        "POST",
        "/api/admin/updates/delete",
        Some(json!({"id": id})),
    )
    .await;
    assert_eq!(again.status, 404, "{}", again.text);
    assert_eq!(
        s.call(None, "GET", "/api/updates", None).await.json["updates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // A member and a signed-out visitor cannot write or see the admin list.
    for sid in [Some(member.as_str()), None] {
        let r = admin(
            &s,
            sid,
            "POST",
            "/api/admin/updates",
            Some(json!({"title": "nope"})),
        )
        .await;
        assert!(
            r.status == 401 || r.status == 403,
            "{} {}",
            r.status,
            r.text
        );
        let r = admin(&s, sid, "GET", "/api/admin/updates", None).await;
        assert!(
            r.status == 401 || r.status == 403,
            "{} {}",
            r.status,
            r.text
        );
    }
    assert_eq!(
        s.call(None, "GET", "/api/updates", None).await.json["updates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
