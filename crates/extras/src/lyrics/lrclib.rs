//! lrclib.net: `/api/get` by tag and duration, then `/api/search` by tag alone.

use super::lrc;
use formalmusic_api::LyricLine;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(untagged)]
enum Body {
    Many(Vec<Entry>),
    One(Entry),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    synced_lyrics: Option<String>,
}

/// Handles `/api/get`'s single object and `/api/search`'s array alike: the
/// first entry whose `syncedLyrics` parses with usable timing. `plainLyrics`
/// is never read. A body that is not JSON, or has nothing usable, is empty.
pub(crate) fn pick_synced(body: &str) -> Vec<LyricLine> {
    let entries = match serde_json::from_str::<Body>(body) {
        Ok(Body::Many(entries)) => entries,
        Ok(Body::One(entry)) => vec![entry],
        Err(_) => return Vec::new(),
    };
    entries
        .into_iter()
        .filter_map(|e| e.synced_lyrics)
        .map(|text| lrc::parse(&text))
        .find(|lines| lrc::has_usable_timing(lines))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_response_is_one_object() {
        let lines = pick_synced(include_str!("../../fixtures/lrclib_get.json"));
        assert_eq!(lines.len(), 8);
        assert_eq!(
            lines[0].text,
            "I have this thing where I get older but just never wiser"
        );
        assert_eq!(lines[0].start_ms, 5_390);
        assert!(lines[0].words.is_empty());
    }

    #[test]
    fn search_response_takes_the_first_usable_entry() {
        let body = r#"[
            {"syncedLyrics":null,"plainLyrics":"x"},
            {"syncedLyrics":""},
            {"syncedLyrics":"no stamps here"},
            {"syncedLyrics":"[00:03.00]third"}]"#;
        let lines = pick_synced(body);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "third");
    }

    #[test]
    fn search_fixture_parses() {
        let lines = pick_synced(include_str!("../../fixtures/lrclib_search.json"));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn nothing_usable_is_empty() {
        assert!(pick_synced("not json").is_empty());
        assert!(pick_synced(r#"{"plainLyrics":"only plain"}"#).is_empty());
        assert!(
            pick_synced(r#"{"message":"Failed to find specified track","statusCode":404}"#)
                .is_empty()
        );
        assert!(pick_synced("[]").is_empty());
    }
}
