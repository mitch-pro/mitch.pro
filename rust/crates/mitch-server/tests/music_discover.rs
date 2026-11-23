//! Discover: genres are listed, unknown genres 404, and the page answers when
//! the music source (yt-dlp) is absent, as it is here ("top songs" is empty).
//! Nothing in the response comes from what accounts keep or play.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;

#[tokio::test(flavor = "multi_thread")]
async fn discover_lists_genres_and_knows_nothing_about_listeners() {
    let s = TestServer::start(&[], "/api/tester/status", 403).await;
    let me = s.session_for("test1@example.com");
    // Songs several accounts keep must not show up anywhere in Discover.
    {
        let db = s.db();
        for who in ["test1", "test2", "test3"] {
            db.execute(
                "INSERT INTO music_favorites (email, video_id, title, artist, duration_s, added_at) \
                 VALUES (?1, 'aaaaaaaaaaa', 'Shared Song', 'Shared Artist', 200, 1)",
                [format!("{who}@example.com")],
            )
            .unwrap();
        }
    }
    let r = s.call(Some(&me), "GET", "/api/music/discover", None).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(r.json["genres"].as_array().unwrap().len() >= 10);
    assert!(r.json.get("popular").is_none(), "{}", r.text);
    assert!(!r.text.contains("Shared Song"));

    let unknown = s
        .call(Some(&me), "GET", "/api/music/discover?genre=nope", None)
        .await;
    assert_eq!(unknown.status, 404);
    let anon = s.call(None, "GET", "/api/music/discover", None).await;
    assert!([401, 403].contains(&anon.status), "{}", anon.status);
}
