//! Hits the real providers. Run with `cargo test -p formalmusic-extras --test live -- --ignored`.

use formalmusic_extras::{AnimatedCovers, Lyricist, LyricsRequest};
use std::time::Duration;

#[tokio::test]
#[ignore = "needs network"]
async fn word_synced_lyrics_end_to_end() {
    let cache = tempfile::tempdir().unwrap();
    let lyricist = Lyricist::with_cache_dir(cache.path().to_owned()).unwrap();
    let request = LyricsRequest {
        title: "Anti-Hero".into(),
        artists: vec!["Taylor Swift".into()],
        album: Some("Midnights".into()),
        duration_ms: Some(200_690),
        youtube_music: None,
    };

    let lyrics = lyricist
        .lyrics(&request)
        .await
        .unwrap()
        .expect("Apple Music has this track");
    assert_eq!(lyrics.source.as_deref(), Some("Apple Music"));
    assert!(lyrics.synced && lyrics.word_synced);
    assert!(lyrics.lines.len() > 30, "{} lines", lyrics.lines.len());
    let first = &lyrics.lines[0];
    assert!(
        first.words.len() > 5 && first.words.iter().all(|w| w.end_ms > w.start_ms),
        "{first:?}"
    );

    let key = "taylor-swift-anti-hero-midnights-201.json";
    for _ in 0..50 {
        if cache.path().join(key).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        cache.path().join(key).exists(),
        "the finished race writes the cache"
    );
    assert_eq!(lyricist.lyrics(&request).await.unwrap().unwrap(), lyrics);
}

#[tokio::test]
#[ignore = "needs network"]
async fn animated_cover_end_to_end() {
    let cache = tempfile::tempdir().unwrap();
    let covers = AnimatedCovers::with_cache_dir(cache.path().to_owned()).unwrap();

    let path = covers
        .animated_cover("Taylor Swift", "The Life of a Showgirl")
        .await
        .unwrap()
        .expect("Apple Music has an animated cover for this album");
    assert_eq!(
        path,
        cache.path().join("taylor-swift-the-life-of-a-showgirl.mp4")
    );
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > 100_000, "{} bytes", bytes.len());
    assert_eq!(&bytes[4..8], b"ftyp");

    assert_eq!(
        covers
            .animated_cover("Taylor Swift", "The Life of a Showgirl")
            .await
            .unwrap(),
        Some(path)
    );
    assert_eq!(
        covers
            .animated_cover("Nobody Real Ever", "Not An Album")
            .await
            .unwrap(),
        None
    );
}
