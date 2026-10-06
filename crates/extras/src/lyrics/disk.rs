//! The lyrics cache. A hit (`<key>.json`) is kept forever. A miss
//! (`<key>.miss`, an empty marker) is trusted for a week, after which the
//! providers are asked again.

use crate::cache::{slug, temp_sibling};
use formalmusic_api::Lyrics;
use std::path::Path;
use std::time::{Duration, SystemTime};

pub(crate) const MISS_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, PartialEq)]
pub(crate) enum Cached {
    Hit(Lyrics),
    Miss,
    Absent,
}

/// `artist title album` slug plus the length in whole seconds, so a remaster
/// with a different length gets its own entry.
pub(crate) fn key(artist: &str, title: &str, album: &str, duration_ms: Option<u64>) -> String {
    let seconds = duration_ms.map_or(0, |ms| (ms as f64 / 1000.0).round() as u64);
    format!("{}-{seconds}", slug(&[artist, title, album]))
}

pub(crate) async fn read(dir: &Path, key: &str) -> Cached {
    if let Ok(bytes) = tokio::fs::read(dir.join(format!("{key}.json"))).await {
        // A corrupt file reads as absent, same as no file.
        if let Ok(lyrics) = serde_json::from_slice::<Lyrics>(&bytes)
            && !lyrics.lines.is_empty()
        {
            return Cached::Hit(lyrics);
        }
    }
    let marker = tokio::fs::metadata(dir.join(format!("{key}.miss"))).await;
    let fresh = marker
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < MISS_TTL);
    if fresh { Cached::Miss } else { Cached::Absent }
}

pub(crate) async fn write_hit(dir: &Path, key: &str, lyrics: &Lyrics) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(lyrics).map_err(std::io::Error::other)?;
    write_atomic(&dir.join(format!("{key}.json")), &bytes).await
}

pub(crate) async fn write_miss(dir: &Path, key: &str) -> std::io::Result<()> {
    write_atomic(&dir.join(format!("{key}.miss")), b"").await
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = temp_sibling(path, "tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await.inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use formalmusic_api::LyricLine;

    fn lyrics() -> Lyrics {
        Lyrics {
            source: Some("Apple Music".into()),
            lines: vec![LyricLine {
                start_ms: 1,
                text: "a".into(),
                ..Default::default()
            }],
            synced: true,
            word_synced: false,
        }
    }

    #[test]
    fn key_rounds_the_length_to_seconds() {
        assert_eq!(
            key("Taylor Swift", "Anti-Hero", "Midnights", Some(200_690)),
            "taylor-swift-anti-hero-midnights-201"
        );
        assert_eq!(key("A", "B", "", None), "a-b-0");
    }

    #[tokio::test]
    async fn hit_round_trips_and_wins_over_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path(), "k").await, Cached::Absent);
        write_miss(dir.path(), "k").await.unwrap();
        assert_eq!(read(dir.path(), "k").await, Cached::Miss);
        write_hit(dir.path(), "k", &lyrics()).await.unwrap();
        assert_eq!(read(dir.path(), "k").await, Cached::Hit(lyrics()));
    }

    #[tokio::test]
    async fn corrupt_json_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("k.json"), "{nope").unwrap();
        assert_eq!(read(dir.path(), "k").await, Cached::Absent);
    }

    #[tokio::test]
    async fn marker_expires_after_seven_days() {
        let dir = tempfile::tempdir().unwrap();
        write_miss(dir.path(), "k").await.unwrap();
        let file = std::fs::File::options()
            .write(true)
            .open(dir.path().join("k.miss"))
            .unwrap();
        file.set_modified(SystemTime::now() - MISS_TTL - Duration::from_secs(60))
            .unwrap();
        assert_eq!(read(dir.path(), "k").await, Cached::Absent);
        file.set_modified(SystemTime::now() - MISS_TTL + Duration::from_secs(3600))
            .unwrap();
        assert_eq!(read(dir.path(), "k").await, Cached::Miss);
    }
}
