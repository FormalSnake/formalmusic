//! Requests against the real API, anonymous. Ignored by default; the weekly
//! maintenance run executes them with
//! `cargo test -p formalmusic-innertube -- --ignored` to catch YouTube
//! changing shape before users do.

use formalmusic_innertube::Client;
use formalmusic_innertube::api::*;

fn client() -> Client {
    Client::anonymous().unwrap()
}

fn all_items(sections: &[Section]) -> impl Iterator<Item = &Item> {
    sections.iter().flat_map(|s| &s.items)
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_home() {
    let client = client();
    let home = client.browse(BrowseTarget::Home).await.unwrap();
    assert!(!home.chips.is_empty(), "home lost its chips");
    assert!(home.sections.len() >= 2);
    assert!(all_items(&home.sections).count() >= 10);
    // Continuation tokens are bound to the visitor id, so the same client
    // has to follow them.
    let more = client
        .continuation(&home.continuation.expect("home continuation"))
        .await
        .unwrap();
    assert!(!more.sections.is_empty());
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_search() {
    let client = client();
    let all = client.search("daft punk", None).await.unwrap();
    assert!(
        all_items(&all.sections)
            .any(|i| matches!(i, Item::Artist { name, .. } if name == "Daft Punk"))
    );

    let songs = client
        .search("daft punk", Some(SearchFilter::Songs))
        .await
        .unwrap();
    let tracks = all_items(&songs.sections)
        .filter(|i| matches!(i, Item::Track(t) if t.video_id.len() == 11))
        .count();
    assert!(tracks >= 10);
    let more = client
        .continuation(&songs.continuation.expect("songs continuation"))
        .await
        .unwrap();
    assert!(!more.items.is_empty());

    let suggestions = client.suggestions("daft p").await.unwrap();
    assert!(
        suggestions
            .iter()
            .any(|s| matches!(s, Suggestion::Query { .. }))
    );
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_album() {
    let album = client()
        .browse(BrowseTarget::Album("MPREb_K8qWMWVqXGi".into()))
        .await
        .unwrap();
    assert!(
        matches!(&album.header, Some(Header::Detail { title, playlist_id: Some(_), .. }) if title == "Random Access Memories")
    );
    let tracks = album.sections[0]
        .items
        .iter()
        .filter(|i| matches!(i, Item::Track(t) if t.duration_ms.is_some()))
        .count();
    assert_eq!(tracks, 13);
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_next() {
    let client = client();
    let radio = client.radio("IluRBvnYMoY").await.unwrap();
    assert!(radio.tracks.len() >= 20);
    assert!(radio.related_browse_id.is_some());
    let more = client
        .next_continuation(
            radio.playlist_id.as_deref().unwrap(),
            &radio.continuation.expect("radio continuation"),
        )
        .await
        .unwrap();
    assert!(!more.tracks.is_empty());

    let related = client
        .related(radio.related_browse_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(!related.sections.is_empty());
    let lyrics = client.lyrics("IluRBvnYMoY").await.unwrap().expect("lyrics");
    assert!(lyrics.lines.len() > 10);
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_playback_tracking() {
    let tracking = client().playback_tracking("IluRBvnYMoY").await.unwrap();
    assert!(tracking.playback_url.starts_with("https://"));
    assert!(tracking.loudness_db.is_some());
}
