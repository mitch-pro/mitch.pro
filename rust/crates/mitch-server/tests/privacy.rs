//! Privacy contracts against a real local server (docs/privacy.md):
//! - a signed-in visitor's IP address, the pages they open and their presence
//!   never reach the database or the data directory;
//! - presence still works, in memory;
//! - data older versions wrote (ping log, last-known IPs, search log, IPs in
//!   cheat logs and IP bans) is removed on startup;
//! - explicit network bans still work after their IPs are hashed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

const MEMBER: &str = "member@example.com";
const VISITOR_IP: &str = "203.0.113.77";
const SECRET_PAGE: &str = "secret-xyz-page";

/// Every text value in every table, plus every non-database file under `dir`.
fn everything_stored(dir: &Path) -> String {
    let db = rusqlite::Connection::open_with_flags(
        dir.join("data/mitchpro.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let mut out = String::new();
    for table in tables {
        let mut stmt = db.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let cols = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for i in 0..cols {
                if let Ok(rusqlite::types::ValueRef::Text(t)) = row.get_ref(i) {
                    out.push_str(&String::from_utf8_lossy(t));
                    out.push('\n');
                }
            }
        }
    }
    fn walk(path: &Path, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name == "webserver" || name.starts_with("mitchpro.db") {
                continue;
            }
            if p.is_dir() {
                walk(&p, out);
            } else if let Ok(text) = std::fs::read(&p) {
                out.push_str(&String::from_utf8_lossy(&text));
            }
        }
    }
    walk(dir, &mut out);
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn ip_pages_and_presence_are_not_stored() {
    let s = TestServer::start(&[], "/api/tester/status", 403).await;
    let sid = s.session_for(MEMBER);
    let origin = s.url.clone();
    let headers = [
        ("X-Forwarded-For", VISITOR_IP),
        ("X-Real-IP", VISITOR_IP),
        ("Origin", origin.as_str()),
    ];

    let r = s
        .call_with(Some(&sid), "GET", "/api/members", None, &headers)
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let r = s
        .call_with(
            Some(&sid),
            "POST",
            "/api/ping",
            Some(json!({ "page": format!("/tools/{SECRET_PAGE}/"), "category": "Utilities" })),
            &headers,
        )
        .await;
    assert!(r.status < 400, "ping: {} {}", r.status, r.text);
    let r = s
        .call_with(
            Some(&sid),
            "POST",
            "/api/presence/heartbeat",
            Some(json!({
                "clientId": "tab-1",
                "seq": 1,
                "page": format!("/games/{SECRET_PAGE}/"),
                "activity": format!("Playing {SECRET_PAGE}"),
                "visible": true,
                "focused": true,
                "shareActivity": true,
                "interaction": 1
            })),
            &headers,
        )
        .await;
    assert_eq!(r.status, 200, "heartbeat: {}", r.text);

    // Presence works: the member sees their own activity, from memory.
    let r = s
        .call_with(Some(&sid), "GET", "/api/members", None, &headers)
        .await;
    let me = r.json["members"]
        .as_array()
        .and_then(|m| {
            m.iter()
                .find(|m| m["email"] == MEMBER || m["online"] == true)
        })
        .cloned()
        .unwrap_or_default();
    assert_eq!(me["online"], true, "{}", r.text);

    // Let the background log writer flush anything it was going to write.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let stored = everything_stored(&s.dir);
    assert!(!stored.contains(VISITOR_IP), "visitor IP was stored");
    assert!(!stored.contains(SECRET_PAGE), "visited page was stored");
    assert!(!stored.contains("last_known_ips"));
}

#[tokio::test(flavor = "multi_thread")]
async fn startup_removes_retired_personal_data_and_keeps_network_bans() {
    let legacy_ip = "198.51.100.9";
    let banned_ip = "198.51.100.66";
    let sessions = json!([{ "id": "sid", "timestamp": "1", "page": "/games/x/", "ip": legacy_ip }]);
    let last_known = json!({ MEMBER: legacy_ip });
    let search = json!([{ "query": "zq-search-probe-8472", "user": MEMBER, "ts": 1 }]);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let cheats = json!([
        { "email": MEMBER, "game": "g", "details": "d", "ts": now, "ip": legacy_ip },
        // Past the 90-day retention: dropped during the move.
        { "email": "old@example.com", "game": "g", "details": "d", "ts": 1, "ip": legacy_ip }
    ]);
    let bans = json!({
        legacy_ip: { "reason": "old automatic ban", "by": "admin", "email": MEMBER },
        banned_ip: { "reason": "explicit network ban", "by": "admin", "email": "" }
    });
    let s = TestServer::start(
        &[
            ("sessions.json", &sessions.to_string()),
            ("last_known_ips.json", &last_known.to_string()),
            ("search_intent.json", &search.to_string()),
            ("cheat_logs.json", &cheats.to_string()),
            ("banned_ips.json", &bans.to_string()),
        ],
        "/api/tester/status",
        403,
    )
    .await;

    for name in [
        "sessions.json",
        "last_known_ips.json",
        "search_intent.json",
        "cheat_logs.json",
        "banned_ips.json",
    ] {
        assert!(!s.dir.join("data").join(name).exists(), "{name} kept");
    }
    let stored = everything_stored(&s.dir);
    assert!(!stored.contains(legacy_ip), "legacy IP kept");
    assert!(!stored.contains(banned_ip), "banned IP stored raw");
    assert!(!stored.contains("zq-search-probe-8472"), "search log kept");

    let db = rusqlite::Connection::open(s.dir.join("data/mitchpro.db")).unwrap();
    let cheat_rows: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM cheat_logs WHERE email = ?1",
            [MEMBER],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cheat_rows, 1, "cheat log entry was not migrated");
    let total: i64 = db
        .query_row("SELECT COUNT(*) FROM cheat_logs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 1, "expired cheat log entry was kept");
    let bans: i64 = db
        .query_row("SELECT COUNT(*) FROM banned_ips", [], |r| r.get(0))
        .unwrap();
    assert_eq!(bans, 1, "only the explicit network ban should remain");

    // The hashed explicit ban is still enforced; the old account-linked IP isn't.
    let r = s
        .call_with(
            None,
            "GET",
            "/api/members",
            None,
            &[("X-Forwarded-For", banned_ip)],
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text);
    assert_eq!(r.json["banned"], true);
    let r = s
        .call_with(
            None,
            "GET",
            "/api/members",
            None,
            &[("X-Forwarded-For", legacy_ip)],
        )
        .await;
    assert_ne!(r.json["banned"], true, "{}", r.text);

    // The retired search-log endpoint is gone.
    let r = s.call(None, "GET", "/api/admin/search-logs", None).await;
    assert!(r.json.get("logs").is_none(), "{}", r.text);
}
