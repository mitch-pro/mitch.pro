//! The frontend is loaded into memory when the server starts and is not read
//! from disk again, so a deploy that swaps files under a running server (old
//! backend, new frontend) cannot change what it serves. Folders the server writes
//! into (`backgrounds/`) are still read from disk.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;

#[tokio::test(flavor = "multi_thread")]
async fn the_frontend_is_served_from_the_startup_snapshot() {
    let s = TestServer::start_with_webroot(
        &[
            ("hello.txt", "version one"),
            ("backgrounds/keep.txt", "x"),
            ("games/some-game/page.txt", "game one"),
            ("game-portal/portal.txt", "portal one"),
        ],
        &[],
        "/api/tester/status",
        403,
    )
    .await;
    let root = s.webroot();
    let me = s.session_for("test1@example.com");
    let get = |path: &'static str| {
        let (s, me) = (&s, me.clone());
        async move { s.call(Some(&me), "GET", path, None).await }
    };
    let first = get("/hello.txt").await;
    assert_eq!(first.text, "version one", "status {}", first.status);

    // A deploy replaces a file, adds one, and deletes another.
    std::fs::write(root.join("hello.txt"), "version two").unwrap();
    std::fs::write(root.join("added-later.txt"), "new").unwrap();
    // The running server keeps serving what it loaded...
    assert_eq!(get("/hello.txt").await.text, "version one");
    // ...and does not pick up files that appeared afterwards.
    assert_eq!(get("/added-later.txt").await.status, 404);

    // Even after the file is deleted from disk, it is still served from memory.
    std::fs::remove_file(root.join("hello.txt")).unwrap();
    assert_eq!(get("/hello.txt").await.text, "version one");

    // A game's own folder is not held in memory: it is read from disk as it
    // changes. The game portal's files are held like the rest of the frontend.
    std::fs::write(root.join("games/some-game/page.txt"), "game two").unwrap();
    std::fs::write(root.join("game-portal/portal.txt"), "portal two").unwrap();
    assert_eq!(get("/games/some-game/page.txt").await.text, "game two");
    assert_eq!(get("/game-portal/portal.txt").await.text, "portal one");

    // Live folders (uploads, backgrounds) are read from disk as they change.
    std::fs::write(root.join("backgrounds/live.txt"), "uploaded").unwrap();
    assert_eq!(get("/backgrounds/live.txt").await.text, "uploaded");
}

/// With `MITCH_STATIC_LIVE=1` or `MITCH_DEV=1` nothing is snapshotted: edits and
/// new files show up on the next request.
#[tokio::test(flavor = "multi_thread")]
async fn live_mode_reads_the_disk_on_every_request() {
    for flag in ["MITCH_STATIC_LIVE", "MITCH_DEV"] {
        let s = TestServer::start_with_env(
            &[("hello.txt", "version one")],
            &[],
            &[(flag, "1")],
            "/api/tester/status",
            403,
        )
        .await;
        let root = s.webroot();
        let me = s.session_for("test1@example.com");
        let get = |path: &'static str| {
            let (s, me) = (&s, me.clone());
            async move { s.call(Some(&me), "GET", path, None).await }
        };
        assert_eq!(get("/hello.txt").await.text, "version one", "{flag}");
        std::fs::write(root.join("hello.txt"), "version two").unwrap();
        std::fs::write(root.join("added-later.txt"), "new").unwrap();
        assert_eq!(get("/hello.txt").await.text, "version two", "{flag}");
        assert_eq!(get("/added-later.txt").await.text, "new", "{flag}");
    }
}
