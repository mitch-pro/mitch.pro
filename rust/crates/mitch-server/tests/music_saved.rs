//! Saved songs: a song someone keeps is served from the audio store with
//! ranges, and the storage summary reports their quota. Uses a real local
//! server and fixture files; nothing contacts YouTube.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;

const ID: &str = "aaaaaaaaaaa";

#[tokio::test(flavor = "multi_thread")]
async fn a_kept_song_plays_from_the_store_and_counts_toward_the_quota() {
    let s = TestServer::start(&[], "/api/tester/status", 403).await;
    let me = s.session_for("test1@example.com");
    let data: Vec<u8> = (0..200u8).collect();
    let store = s.data_dir().join("music-audio");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join(format!("{ID}.m4a")), &data).unwrap();
    {
        let db = s.db();
        db.execute(
            "INSERT INTO music_audio_files (video_id, size, mime, ext, stored_at) VALUES (?1, 200, 'audio/mp4', 'm4a', 1)",
            [ID],
        )
        .unwrap();
        db.execute(
            "INSERT INTO music_favorites (email, video_id, title, artist, duration_s, added_at) \
             VALUES ('test1@example.com', ?1, 'Song', 'Artist', 200, 1)",
            [ID],
        )
        .unwrap();
    }

    let url = format!("/api/music/stream?id={ID}");
    let ranged = s
        .call_with(Some(&me), "GET", &url, None, &[("Range", "bytes=10-19")])
        .await;
    assert_eq!(ranged.status, 206, "{}", ranged.text);
    assert_eq!(ranged.body, data[10..20].to_vec());
    let whole = s.call(Some(&me), "GET", &url, None).await;
    assert_eq!(whole.status, 200);
    assert_eq!(whole.body.len(), 200);
    let past = s
        .call_with(Some(&me), "GET", &url, None, &[("Range", "bytes=500-")])
        .await;
    assert_eq!(past.status, 416);

    let summary = s.call(Some(&me), "GET", "/api/music/storage", None).await;
    assert_eq!(summary.status, 200, "{}", summary.text);
    assert_eq!(summary.json["usedBytes"], 200);
    assert_eq!(summary.json["limitBytes"], 1024 * 1024 * 1024);
    assert_eq!(summary.json["songs"], 1);
    assert_eq!(summary.json["saved"], 1);

    // Signed out visitors get nothing.
    let anon = s.call(None, "GET", &url, None).await;
    assert!([401, 403].contains(&anon.status), "{}", anon.status);
}
