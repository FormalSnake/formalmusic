use super::*;
use crate::mock::{self, Mock, route};
use formalmusic_api::LyricWord;
use std::time::Instant;

const SEARCH: &str = include_str!("../../fixtures/itunes_search.json");
const APPLE: &str = include_str!("../../fixtures/paxsenix_apple.json");
const LRCLIB: &str = include_str!("../../fixtures/lrclib_get.json");

fn request() -> LyricsRequest {
    LyricsRequest {
        title: "Anti-Hero".into(),
        artists: vec!["Taylor Swift".into()],
        album: Some("Midnights".into()),
        duration_ms: Some(200_690),
        youtube_music: None,
    }
}

fn lyricist(dir: &tempfile::TempDir, base: &str, lrclib: &str) -> Lyricist {
    let endpoints = Endpoints {
        itunes: base.into(),
        paxsenix: base.into(),
        lrclib: lrclib.into(),
    };
    Lyricist::build(dir.path().to_owned(), endpoints).unwrap()
}

fn files(dir: &tempfile::TempDir) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir.path())
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The race outlives the call that wins it, so cache writes land a moment later.
async fn settle(dir: &tempfile::TempDir, want: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let found = files(dir);
        if found.len() >= want || Instant::now() > deadline {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn line(start_ms: u64, words: usize) -> LyricLine {
    let word = |i| LyricWord {
        start_ms: start_ms + i,
        end_ms: start_ms + i + 1,
        text: "w".into(),
        joins_next: false,
    };
    LyricLine {
        start_ms,
        words: (0..words as u64).map(word).collect(),
        ..Default::default()
    }
}

fn lyrics_of(lines: Vec<LyricLine>) -> Lyrics {
    Lyrics {
        source: None,
        lines,
        synced: true,
        word_synced: false,
    }
}

fn ytm(synced: bool) -> Lyrics {
    let l = LyricLine {
        start_ms: if synced { 1000 } else { 0 },
        text: "from ytm".into(),
        ..Default::default()
    };
    Lyrics {
        source: Some("LyricFind".into()),
        lines: vec![l],
        synced,
        word_synced: false,
    }
}

#[test]
fn quality_counts_word_timing() {
    assert_eq!(quality(&[]), 0);
    assert_eq!(quality(&[line(0, 0)]), 1);
    assert_eq!(quality(&[line(0, 1)]), 1);
    assert_eq!(quality(&[line(0, 1), line(1, 2)]), 2);
}

#[test]
fn best_quality_wins_and_the_earlier_provider_takes_ties() {
    let pick = |c| pick_best(c).map(|(p, _)| p);
    let a = (Provider::Apple, lyrics_of(vec![line(0, 0)]));
    let l = (Provider::Lrclib, lyrics_of(vec![line(0, 3)]));
    let y = (Provider::YoutubeMusic, lyrics_of(vec![line(0, 0)]));
    assert_eq!(
        pick(vec![a.clone(), l.clone(), y.clone()]),
        Some(Provider::Lrclib)
    );
    assert_eq!(pick(vec![a.clone(), y.clone()]), Some(Provider::Apple));
    assert_eq!(
        pick(vec![(Provider::Apple, lyrics_of(vec![])), y]),
        Some(Provider::YoutubeMusic)
    );
    assert_eq!(pick(vec![(Provider::Apple, lyrics_of(vec![]))]), None);
}

#[tokio::test]
async fn word_timing_wins_and_is_cached_for_good() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![
        route("/search", 200, SEARCH),
        route("/apple-music/lyrics?id=1649434293", 200, APPLE),
        route("/api/", 404, ""),
    ])
    .await;
    let l = lyricist(&dir, &mock.base, &mock.base);

    let found = l.lyrics(&request()).await.unwrap().unwrap();
    assert_eq!(found.source.as_deref(), Some("Apple Music"));
    assert!(found.synced && found.word_synced);
    assert!(found.lines[0].words.len() > 1);

    assert_eq!(
        settle(&dir, 1).await,
        ["taylor-swift-anti-hero-midnights-201.json"]
    );
    let before = mock.targets().len();
    let again = l.lyrics(&request()).await.unwrap().unwrap();
    assert_eq!(again, found);
    assert_eq!(
        mock.targets().len(),
        before,
        "second lookup is served from disk"
    );
}

#[tokio::test]
async fn lrclib_answers_when_apple_has_no_match() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![
        route("/search", 200, r#"{"results":[]}"#),
        route("/api/get", 200, LRCLIB),
    ])
    .await;
    let found = lyricist(&dir, &mock.base, &mock.base)
        .lyrics(&request())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.source.as_deref(), Some("LRCLIB"));
    assert!(found.synced && !found.word_synced);
    assert_eq!(files(&dir), ["taylor-swift-anti-hero-midnights-201.json"]);

    let get = mock
        .targets()
        .into_iter()
        .find(|t| t.starts_with("/api/get"))
        .unwrap();
    assert!(
        get.contains("track_name=Anti-Hero")
            && get.contains("album_name=Midnights")
            && get.contains("duration=201"),
        "{get}"
    );
}

#[tokio::test]
async fn lrclib_search_runs_when_get_misses() {
    let dir = tempfile::tempdir().unwrap();
    let search = include_str!("../../fixtures/lrclib_search.json");
    let mock = Mock::start(vec![
        route("/search", 200, r#"{"results":[]}"#),
        route("/api/get", 404, r#"{"statusCode":404}"#),
        route("/api/search", 200, search),
    ])
    .await;
    let found = lyricist(&dir, &mock.base, &mock.base)
        .lyrics(&request())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.lines.len(), 3);
}

#[tokio::test]
async fn nothing_found_leaves_a_miss_marker_that_stops_the_network() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![route("/search", 200, r#"{"results":[]}"#)]).await;
    let l = lyricist(&dir, &mock.base, &mock.base);
    assert_eq!(l.lyrics(&request()).await.unwrap(), None);
    assert_eq!(files(&dir), ["taylor-swift-anti-hero-midnights-201.miss"]);

    let before = mock.targets().len();
    assert_eq!(l.lyrics(&request()).await.unwrap(), None);
    assert_eq!(mock.targets().len(), before);
}

#[tokio::test]
async fn youtube_music_is_one_more_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![route("/search", 200, r#"{"results":[]}"#)]).await;
    let l = lyricist(&dir, &mock.base, &mock.base);

    let mut req = request();
    req.youtube_music = Some(ytm(true));
    let found = l.lyrics(&req).await.unwrap().unwrap();
    assert_eq!(found.source.as_deref(), Some("LyricFind"));
    // Only network providers earn a cached hit; their silence is cached.
    assert_eq!(files(&dir), ["taylor-swift-anti-hero-midnights-201.miss"]);

    req.youtube_music = Some(ytm(false));
    let plain = l.lyrics(&req).await.unwrap().unwrap();
    assert!(!plain.synced);
}

#[tokio::test]
async fn plain_youtube_lyrics_do_not_beat_a_synced_provider() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![
        route("/search", 200, r#"{"results":[]}"#),
        route("/api/get", 200, LRCLIB),
    ])
    .await;
    let mut req = request();
    req.youtube_music = Some(ytm(false));
    let found = lyricist(&dir, &mock.base, &mock.base)
        .lyrics(&req)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.source.as_deref(), Some("LRCLIB"));
}

#[tokio::test]
async fn a_failed_provider_with_no_answer_is_an_error_and_caches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![route("/search", 200, r#"{"results":[]}"#)]).await;
    let dead = mock::dead_base().await;
    let l = lyricist(&dir, &mock.base, &dead);
    assert!(l.lyrics(&request()).await.is_err());
    assert!(files(&dir).is_empty());
}

#[tokio::test]
async fn server_errors_count_as_failures_but_not_found_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![route("/search", 503, ""), route("/api/", 404, "")]).await;
    assert!(
        lyricist(&dir, &mock.base, &mock.base)
            .lyrics(&request())
            .await
            .is_err()
    );
    assert!(files(&dir).is_empty());
}

#[tokio::test]
async fn a_failure_alongside_a_winner_keeps_the_result_session_only() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![
        route("/search", 200, SEARCH),
        route("/apple-music/lyrics", 200, APPLE),
    ])
    .await;
    let dead = mock::dead_base().await;
    let l = lyricist(&dir, &mock.base, &dead);
    let found = l.lyrics(&request()).await.unwrap().unwrap();
    assert!(found.word_synced);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(files(&dir).is_empty(), "{:?}", files(&dir));
}

#[tokio::test]
async fn a_failed_provider_still_yields_the_cached_youtube_answer_without_caching() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(vec![route("/search", 200, r#"{"results":[]}"#)]).await;
    let dead = mock::dead_base().await;
    let mut req = request();
    req.youtube_music = Some(ytm(true));
    let found = lyricist(&dir, &mock.base, &dead)
        .lyrics(&req)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.source.as_deref(), Some("LyricFind"));
    assert!(files(&dir).is_empty());
}
