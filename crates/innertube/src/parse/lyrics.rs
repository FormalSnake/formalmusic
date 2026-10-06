//! Lyrics from the player page's Lyrics tab. `WEB_REMIX` only ever returns
//! plain text in a `musicDescriptionShelfRenderer`; the `ANDROID_MUSIC`
//! client returns a `timedLyricsModel` with per-line cue ranges when the
//! track has synced lyrics, and the same model without them otherwise.

use super::text;
use formalmusic_api::{LyricLine, Lyrics};
use serde_json::Value;

/// `None` when the response has no timed model, such as "Lyrics not
/// available" messages.
pub fn parse_timed(json: &Value) -> Option<Lyrics> {
    let data = &json["contents"]["elementRenderer"]["newElement"]["type"]["componentType"]["model"]
        ["timedLyricsModel"]["lyricsData"];
    let raw = data["timedLyricsData"].as_array()?;
    let mut synced = data["staticLayout"].as_bool() != Some(true);
    let lines: Vec<LyricLine> = raw
        .iter()
        .map(|line| {
            let cue = &line["cueRange"];
            let start = millis(&cue["startTimeMilliseconds"]);
            if start.is_none() {
                synced = false;
            }
            LyricLine {
                start_ms: start.unwrap_or(0),
                end_ms: millis(&cue["endTimeMilliseconds"]),
                text: line["lyricLine"].as_str().unwrap_or_default().to_owned(),
                ..Default::default()
            }
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    let lines = if synced {
        lines
    } else {
        lines
            .into_iter()
            .map(|l| LyricLine {
                start_ms: 0,
                end_ms: None,
                ..l
            })
            .collect()
    };
    Some(Lyrics {
        source: data["sourceMessage"].as_str().map(source_name),
        lines,
        synced,
        word_synced: false,
    })
}

/// Plain lyrics from the `WEB_REMIX` response.
pub fn parse_plain(json: &Value) -> Option<Lyrics> {
    let shelf = json["contents"]["sectionListRenderer"]["contents"]
        .as_array()?
        .iter()
        .find_map(|c| c.get("musicDescriptionShelfRenderer"))?;
    let body = text(&shelf["description"])?;
    let lines = body
        .lines()
        .map(|line| LyricLine {
            start_ms: 0,
            end_ms: None,
            text: line.to_owned(),
            ..Default::default()
        })
        .collect();
    Some(Lyrics {
        source: text(&shelf["footer"]).as_deref().map(source_name),
        lines,
        synced: false,
        word_synced: false,
    })
}

fn source_name(message: &str) -> String {
    message
        .trim()
        .trim_start_matches("Source:")
        .trim()
        .to_owned()
}

fn millis(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn synced_lines_keep_their_cues() {
        let json = json!({"contents": {"elementRenderer": {"newElement": {"type": {"componentType": {"model": {
            "timedLyricsModel": {"lyricsData": {
                "sourceMessage": "Source: Musixmatch",
                "timedLyricsData": [
                    {"lyricLine": "first", "cueRange": {"startTimeMilliseconds": "1200", "endTimeMilliseconds": "3400"}},
                    {"lyricLine": "second", "cueRange": {"startTimeMilliseconds": "3400", "endTimeMilliseconds": "5000"}}
                ]
            }}
        }}}}}}});
        let lyrics = parse_timed(&json).unwrap();
        assert!(lyrics.synced);
        assert_eq!(lyrics.source.as_deref(), Some("Musixmatch"));
        assert_eq!(lyrics.lines[1].start_ms, 3400);
        assert_eq!(lyrics.lines[0].end_ms, Some(3400));
    }
}
