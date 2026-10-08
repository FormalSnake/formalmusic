//! Hits the real providers. Run with `cargo test -p formalmusic-extras --test live -- --ignored`.

use formalmusic_extras::AnimatedCovers;

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
