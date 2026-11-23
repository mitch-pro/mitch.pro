//! Discover: genres are listed, songs several accounts keep show up as
//! popular (and nothing about who keeps them), unknown genres 404. The music
//! source (yt-dlp) is absent here, so "top songs" is empty and the page still
//! answers.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;

#[tokio::test(flavor = "multi_thread")]
async fn discover_lists_genres_and_popular_songs_without_naming_anyone() {
    let s = TestServer::start(&[], "/api/tester/status", 403).await;
    let me = s.session_for("test1@example.com");
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
        db.execute(
            "INSERT INTO music_favorites (email, video_id, title, artist, duration_s, added_at) \
             VALUES ('test1@example.com', 'bbbbbbbbbbb', 'Private Song', 'Solo', 200, 1)",
            [],
        )
        .unwrap();
    }
    let r = s.call(Some(&me), "GET", "/api/music/discover", None).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(r.json["genres"].as_array().unwrap().len() >= 10);
    let popular = r.json["popular"].as_array().unwrap();
    assert_eq!(popular.len(), 1, "{}", r.text);
    assert_eq!(popular[0]["title"], "Shared Song");
    assert!(
        !r.text.contains("example.com"),
        "no account in the response"
    );
    assert!(!r.text.contains("Private Song"));

    let unknown = s
        .call(Some(&me), "GET", "/api/music/discover?genre=nope", None)
        .await;
    assert_eq!(unknown.status, 404);
    let anon = s.call(None, "GET", "/api/music/discover", None).await;
    assert!([401, 403].contains(&anon.status), "{}", anon.status);
}
