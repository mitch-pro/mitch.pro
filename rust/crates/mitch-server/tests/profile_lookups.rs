//! "Block profile lookups": the account itself and staff can still open the
//! profile and find it in search; everyone else (testers included) gets nothing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;
use serde_json::json;

const BLOCKED: &str = "test1@example.com";
const VIEWER: &str = "test2@example.com";
const ADMIN: &str = "admin@example.com";
const TESTER: &str = "tester@example.com";

fn add_profile(s: &TestServer, email: &str, username: &str) {
    s.db()
        .execute(
            "INSERT OR REPLACE INTO profiles (email, data, updated_at) VALUES (?1, ?2, 1)",
            rusqlite::params![
                email,
                json!({"email": email, "username": username, "displayName": username}).to_string()
            ],
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn blocked_profiles_are_visible_only_to_the_owner_and_staff() {
    let admins = json!({"owners": [], "coOwners": [], "admins": [ADMIN]}).to_string();
    let testers = json!([TESTER]).to_string();
    let s = TestServer::start(
        &[("admins.json", &admins), ("testers.json", &testers)],
        "/api/tester/status",
        403,
    )
    .await;
    let blocked = s.session_for(BLOCKED);
    let viewer = s.session_for(VIEWER);
    let admin = s.session_for(ADMIN);
    let tester = s.session_for(TESTER);
    add_profile(&s, BLOCKED, "hidden.person");
    add_profile(&s, VIEWER, "regular.person");
    let origin = [("Origin", s.url.as_str())];
    let profile = |sid: &str| {
        let sid = sid.to_string();
        let s = &s;
        async move {
            s.call(Some(&sid), "GET", "/api/profile/hidden.person", None)
                .await
        }
    };
    let search = |sid: &str| {
        let sid = sid.to_string();
        let s = &s;
        async move {
            let r = s
                .call(Some(&sid), "GET", "/api/members?q=person", None)
                .await;
            assert_eq!(r.status, 200, "{}", r.text);
            r.json["members"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|m| m["handle"].as_str().or(m["username"].as_str()))
                .map(str::to_string)
                .collect::<Vec<_>>()
        }
    };

    // Off by default: everyone can look the profile up.
    assert_eq!(profile(&viewer).await.status, 200);
    assert!(search(&viewer).await.contains(&"hidden.person".to_string()));

    // The owner turns it on (and can read it back).
    let off = s
        .call_with(
            Some(&blocked),
            "POST",
            "/api/me/profile-privacy",
            Some(json!({"blockLookups": "yes"})),
            &origin,
        )
        .await;
    assert_eq!(off.status, 400);
    let on = s
        .call_with(
            Some(&blocked),
            "POST",
            "/api/me/profile-privacy",
            Some(json!({"blockLookups": true})),
            &origin,
        )
        .await;
    assert_eq!(on.status, 200, "{}", on.text);
    let read = s
        .call(Some(&blocked), "GET", "/api/me/profile-privacy", None)
        .await;
    assert_eq!(read.json["blockLookups"], true);

    // Other members and testers: not found, and absent from search.
    for sid in [&viewer, &tester] {
        assert_eq!(profile(sid).await.status, 404);
        assert!(!search(sid).await.contains(&"hidden.person".to_string()));
    }
    // A signed-out visitor too.
    let anon = s
        .call(None, "GET", "/api/profile/hidden.person", None)
        .await;
    assert_eq!(anon.status, 404);
    // The account itself and staff still see it.
    assert_eq!(profile(&blocked).await.status, 200);
    assert_eq!(profile(&admin).await.status, 200);
    assert!(search(&admin).await.contains(&"hidden.person".to_string()));
    assert!(search(&blocked)
        .await
        .contains(&"hidden.person".to_string()));
    // Others are unaffected.
    let other = s
        .call(Some(&viewer), "GET", "/api/profile/regular.person", None)
        .await;
    assert_eq!(other.status, 200);

    // Turning it off restores lookups.
    s.call_with(
        Some(&blocked),
        "POST",
        "/api/me/profile-privacy",
        Some(json!({"blockLookups": false})),
        &origin,
    )
    .await;
    assert_eq!(profile(&viewer).await.status, 200);
}
