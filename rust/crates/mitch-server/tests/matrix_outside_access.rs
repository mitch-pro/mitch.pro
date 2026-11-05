//! Outside (non-student, non-staff) accounts can't use chat until a moderator
//! approves them: Matrix SSO refuses them a token and `/matrix/` serves an
//! "approval needed" page that points to support. Students, approved outside
//! accounts and signed-out visitors keep the normal chat client.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;

const STUDENT: &str = "kid@student.rjuhsd.us";
const OUTSIDE: &str = "visitor@example.com";
const APPROVED: &str = "approved@example.com";
const DENIED: &str = "denied@example.com";

fn approval_status(s: &TestServer, email: &str) -> Option<String> {
    let db = rusqlite::Connection::open(s.dir.join("data/mitchpro.db")).unwrap();
    db.query_row(
        "SELECT status FROM matrix_outside_approvals WHERE email = ?1",
        [email],
        |row| row.get(0),
    )
    .ok()
}

fn decide(s: &TestServer, email: &str, status: &str) {
    let db = rusqlite::Connection::open(s.dir.join("data/mitchpro.db")).unwrap();
    db.execute(
        "INSERT INTO matrix_outside_approvals (email, status, requested_at) VALUES (?1, ?2, 0)",
        rusqlite::params![email, status],
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn unapproved_outside_accounts_cannot_open_chat() {
    let s = TestServer::start(&[], "/api/tester/status", 403).await;
    let student = s.session_for(STUDENT);
    let outside = s.session_for(OUTSIDE);
    let approved = s.session_for(APPROVED);
    let denied = s.session_for(DENIED);
    decide(&s, APPROVED, "approved");
    decide(&s, DENIED, "denied");
    let origin = [("Origin", s.url.as_str())];

    // Unapproved outside account: no token, no client, queued for review.
    let status = s
        .call(Some(&outside), "GET", "/api/matrix/sso-status", None)
        .await;
    assert_eq!(status.json["authenticated"], false, "{}", status.text);
    assert_eq!(status.json["needsApproval"], true, "{}", status.text);
    let login = s
        .call_with(
            Some(&outside),
            "POST",
            "/api/matrix/sso-login",
            Some(serde_json::json!({})),
            &origin,
        )
        .await;
    assert_eq!(login.status, 403, "{}", login.text);
    assert_eq!(login.json["needsApproval"], true, "{}", login.text);
    for page in [
        "/matrix/",
        "/matrix/index.html",
        "/matrix/home/!room:mitch.pro",
    ] {
        let r = s.call(Some(&outside), "GET", page, None).await;
        assert_eq!(r.status, 403, "{page}");
        assert!(r.text.contains("Chat needs approval"), "{page}");
        assert!(r.text.contains("href=\"/support/\""), "{page}");
    }
    assert_eq!(approval_status(&s, OUTSIDE).as_deref(), Some("pending"));

    // Denied outside account: same block, different message, stays denied.
    let r = s.call(Some(&denied), "GET", "/matrix/", None).await;
    assert_eq!(r.status, 403);
    assert!(r.text.contains("didn't approve"));
    let login = s
        .call_with(
            Some(&denied),
            "POST",
            "/api/matrix/sso-login",
            Some(serde_json::json!({})),
            &origin,
        )
        .await;
    assert_eq!(login.status, 403, "{}", login.text);
    assert_eq!(approval_status(&s, DENIED).as_deref(), Some("denied"));

    // Students, approved outside accounts and signed-out visitors get the client.
    for sid in [Some(student.as_str()), Some(approved.as_str()), None] {
        let r = s.call(sid, "GET", "/matrix/", None).await;
        assert_eq!(r.status, 200, "{sid:?}");
        assert!(!r.text.contains("Chat needs approval"), "{sid:?}");
    }
    for sid in [&student, &approved] {
        let status = s
            .call(Some(sid), "GET", "/api/matrix/sso-status", None)
            .await;
        assert_eq!(status.json["authenticated"], true, "{}", status.text);
    }
    assert_eq!(
        approval_status(&s, STUDENT),
        None,
        "students are never queued"
    );
}

/// Matrix accounts are only created by mitch.pro's SSO login (which talks to
/// the homeserver directly), so the public gateway refuses every registration
/// endpoint, signed in or not.
#[tokio::test(flavor = "multi_thread")]
async fn matrix_registration_is_not_proxied() {
    let s = TestServer::start(&[], "/api/tester/status", 403).await;
    let student = s.session_for(STUDENT);
    for (method, path) in [
        ("POST", "/_matrix/client/v3/register"),
        ("POST", "/_matrix/client/v3/register?kind=guest"),
        ("POST", "/_matrix/client/r0/register"),
        (
            "GET",
            "/_matrix/client/v3/register/available?username=someone",
        ),
        ("POST", "/_matrix/client/v3/register/email/requestToken"),
        (
            "GET",
            "/_matrix/client/v1/register/m.login.registration_token/validity?token=x",
        ),
    ] {
        for sid in [None, Some(student.as_str())] {
            let body = (method == "POST").then(|| serde_json::json!({}));
            let r = s.call(sid, method, path, body).await;
            assert_eq!(r.status, 403, "{method} {path}: {}", r.text);
            assert_eq!(r.json["errcode"], "M_FORBIDDEN", "{method} {path}");
        }
    }
}
