//! Apple Music lyrics through paxsenix: an iTunes search finds the track id,
//! paxsenix serves Apple's own timed lyrics for it, with word or syllable
//! stamps, duet voices and backing vocals.

use super::{lrc, score};
use formalmusic_api::{LyricLine, LyricWord};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SearchResults {
    #[serde(default)]
    results: Vec<Song>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Song {
    pub track_id: Option<u64>,
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    track_time_millis: Option<f64>,
}

/// A malformed body reads as no candidates.
pub(crate) fn parse_search(body: &str) -> Vec<Song> {
    serde_json::from_str::<SearchResults>(body)
        .map(|r| r.results)
        .unwrap_or_default()
}

/// The best hit for `query` ("title artist") against the track's length.
/// Length beats a pure text match, but only inside the window.
pub(crate) fn best_song<'a>(
    songs: &'a [Song],
    query: &str,
    track_ms: Option<u64>,
) -> Option<&'a Song> {
    songs
        .iter()
        .filter(|s| s.track_id.is_some())
        .filter_map(|song| {
            let candidate = format!("{} {}", song.track_name, song.artist_name);
            let secs = song.track_time_millis.map(|ms| ms / 1000.0);
            let rank = score::rank(score::match_score(&candidate, query), track_ms, secs)?;
            Some((rank, song))
        })
        .fold(
            None,
            |best: Option<(f64, &Song)>, (rank, song)| match best {
                Some((top, _)) if top >= rank => best,
                _ => Some((rank, song)),
            },
        )
        .map(|(_, song)| song)
}

#[derive(Debug, Deserialize)]
struct Body {
    content: Option<Vec<Row>>,
    lrc: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    timestamp: Option<f64>,
    endtime: Option<f64>,
    text: Option<Vec<Part>>,
    background_text: Option<Vec<Part>>,
    background: Option<bool>,
    opposite_turn: Option<bool>,
    agent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Part {
    text: Option<String>,
    timestamp: Option<f64>,
    endtime: Option<f64>,
    /// The next part joins this one with no space between.
    part: Option<bool>,
}

fn ms(value: Option<f64>) -> Option<u64> {
    value
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v.round() as u64)
}

fn positive(value: Option<f64>) -> bool {
    value.is_some_and(|v| v > 0.0)
}

fn has_timing(rows: &[Row]) -> bool {
    rows.iter().any(|row| {
        let parts = |p: &Option<Vec<Part>>| p.iter().flatten().any(|part| positive(part.timestamp));
        positive(row.timestamp)
            || positive(row.endtime)
            || parts(&row.text)
            || parts(&row.background_text)
    })
}

/// No space before the first part, after a part flagged as continuing, or
/// before a closing punctuation mark.
fn needs_space(text_so_far: &str, previous_continues: bool, next: &str) -> bool {
    const NO_SPACE_BEFORE: [char; 11] = [
        ',', '.', '?', '!', ':', ';', ')', ']', '}', '\'', '\u{2019}',
    ];
    if text_so_far.is_empty() || previous_continues {
        return false;
    }
    next.chars()
        .next()
        .is_some_and(|c| !NO_SPACE_BEFORE.contains(&c))
}

struct Voice<'a> {
    agent: Option<&'a str>,
    opposite_turn: bool,
    background: bool,
}

/// One `text` or `backgroundText` array as a line. The line's text keeps the
/// inserted spaces; each timed part becomes a word with its own text trimmed.
/// `None` when every part is blank.
fn line_from_parts(
    parts: &[Part],
    start_ms: u64,
    end_ms: Option<u64>,
    voice: Voice,
) -> Option<LyricLine> {
    let mut text = String::new();
    let mut words = Vec::new();
    let mut previous_continues = false;
    for part in parts {
        let Some(raw) = part.text.as_deref().filter(|t| !t.trim().is_empty()) else {
            continue;
        };
        let spaced = if needs_space(&text, previous_continues, raw) {
            format!(" {raw}")
        } else {
            raw.to_owned()
        };
        text.push_str(&spaced);
        if let Some(start) = ms(part.timestamp) {
            words.push(LyricWord {
                start_ms: start,
                end_ms: ms(part.endtime).filter(|end| *end > start).unwrap_or(start),
                text: spaced.trim().to_owned(),
                joins_next: part.part == Some(true),
            });
        }
        previous_continues = part.part == Some(true);
    }
    let text = text.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    let mut line = LyricLine {
        start_ms,
        end_ms,
        text,
        words,
        background: voice.background,
        agent: voice.agent.map(str::to_owned),
        opposite_turn: voice.opposite_turn,
    };
    fill_missing_word_ends(&mut line);
    Some(line)
}

/// Parts normally carry their own end. When one does not, it runs to the next
/// part, then to the line's end.
fn fill_missing_word_ends(line: &mut LyricLine) {
    let needs_fill: Vec<bool> = line.words.iter().map(|w| w.end_ms <= w.start_ms).collect();
    if !needs_fill.contains(&true) {
        return;
    }
    let starts: Vec<u64> = line.words.iter().map(|w| w.start_ms).collect();
    let line_end = line.end_ms;
    for (i, word) in line.words.iter_mut().enumerate() {
        if needs_fill[i] {
            word.end_ms = starts
                .get(i + 1)
                .copied()
                .or(line_end.filter(|end| *end > word.start_ms))
                .unwrap_or(word.start_ms + 350);
        }
    }
}

/// A row's `text` is a foreground line unless the row is `background` with no
/// `backgroundText` of its own, in which case it is the background line. A
/// row's `backgroundText` becomes its own background line right after the
/// main one, starting at its first timed part.
fn lines_from_rows(rows: &[Row]) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for row in rows {
        let start = ms(row.timestamp).unwrap_or(0);
        let end = ms(row.endtime);
        let agent = row.agent.as_deref();
        let opposite_turn = row.opposite_turn == Some(true);
        let background_parts = row.background_text.as_deref().unwrap_or_default();
        let main_is_background = row.background == Some(true) && background_parts.is_empty();

        let voice = |background| Voice {
            agent,
            opposite_turn,
            background,
        };
        lines.extend(line_from_parts(
            row.text.as_deref().unwrap_or_default(),
            start,
            end,
            voice(main_is_background),
        ));
        let background_start = background_parts
            .iter()
            .find_map(|p| ms(p.timestamp))
            .unwrap_or(start);
        lines.extend(line_from_parts(
            background_parts,
            background_start,
            end,
            voice(true),
        ));
    }
    lines
}

/// Lines from a paxsenix Apple Music lyrics body. `content` wins when it
/// carries real timing, else the line-synced `lrc` text is parsed. Empty when
/// there is nothing usable.
pub(crate) fn parse_lyrics(body: &str) -> Vec<LyricLine> {
    let Ok(body) = serde_json::from_str::<Body>(body) else {
        return Vec::new();
    };
    let rows = body.content.unwrap_or_default();
    if has_timing(&rows) {
        let lines = lines_from_rows(&rows);
        if lrc::has_usable_timing(&lines) {
            return lines;
        }
    }
    let parsed = lrc::parse(body.lrc.as_deref().unwrap_or_default());
    if lrc::has_usable_timing(&parsed) {
        parsed
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../../fixtures/itunes_search.json");
    const ANTI_HERO: &str = include_str!("../../fixtures/paxsenix_apple.json");
    const BACKGROUND: &str = include_str!("../../fixtures/paxsenix_apple_background.json");
    const DUET: &str = include_str!("../../fixtures/paxsenix_apple_duet.json");

    #[test]
    fn picks_the_studio_cut_by_text_then_length() {
        let songs = parse_search(SEARCH);
        assert_eq!(songs.len(), 8);
        let best = best_song(&songs, "Anti-Hero Taylor Swift", Some(201_000)).unwrap();
        assert_eq!(best.track_id, Some(1649434293));
    }

    #[test]
    fn length_breaks_a_text_tie() {
        let songs = parse_search(
            r#"{"results":[
                {"trackId":1,"trackName":"Song","artistName":"A","trackTimeMillis":190000},
                {"trackId":2,"trackName":"Song","artistName":"A","trackTimeMillis":201000}]}"#,
        );
        assert_eq!(
            best_song(&songs, "Song A", Some(200_000)).unwrap().track_id,
            Some(2)
        );
    }

    #[test]
    fn nothing_clears_the_floor_or_the_window() {
        let songs = parse_search(SEARCH);
        assert!(best_song(&songs, "Completely Different Title Nobody", Some(201_000)).is_none());
        assert!(best_song(&songs, "Anti-Hero Taylor Swift", Some(400_000)).is_none());
    }

    #[test]
    fn malformed_search_is_no_candidates() {
        assert!(parse_search("<html>").is_empty());
        assert!(parse_search("{}").is_empty());
    }

    #[test]
    fn syllable_lyrics_keep_word_timing() {
        let lines = parse_lyrics(ANTI_HERO);
        assert_eq!(lines.len(), 8);
        assert_eq!(
            lines[0].text,
            "I have this thing where I get older, but just never wiser"
        );
        assert_eq!(lines[0].start_ms, 5_347);
        assert_eq!(lines[0].end_ms, Some(10_235));
        let first = &lines[0].words[0];
        assert_eq!(
            (first.text.as_str(), first.start_ms, first.end_ms),
            ("I", 5_347, 5_711)
        );
        assert_eq!(lines[0].agent.as_deref(), Some("v1"));
        assert!(!lines[0].background && !lines[0].opposite_turn);
    }

    #[test]
    fn syllable_text_has_no_space_inside_a_word() {
        let lines = parse_lyrics(ANTI_HERO);
        assert_eq!(lines[1].text, "Midnights become my afternoons");
        let joined: Vec<_> = lines[1]
            .words
            .iter()
            .map(|w| (w.text.as_str(), w.joins_next))
            .collect();
        assert!(joined.contains(&("after", true)), "{joined:?}");
    }

    #[test]
    fn backing_vocals_become_their_own_line() {
        let lines = parse_lyrics(BACKGROUND);
        assert_eq!(lines.len(), 3, "{lines:#?}");
        let (main, back) = (&lines[1], &lines[2]);
        assert!(!main.background);
        assert!(back.background);
        assert_eq!(back.text, "Yes");
        assert_eq!(back.start_ms, 55_644);
    }

    #[test]
    fn duet_voices_carry_agent_and_turn() {
        let lines = parse_lyrics(DUET);
        assert_eq!(lines[0].agent.as_deref(), Some("v2"));
        assert!(lines[0].opposite_turn);
        assert_eq!(lines[1].agent.as_deref(), Some("v1"));
        assert!(!lines[1].opposite_turn);
    }

    #[test]
    fn untimed_content_falls_back_to_lrc() {
        let body =
            r#"{"content":[{"text":[{"text":"hello"}]}],"lrc":"[00:01.00] a\n[00:02.00] b"}"#;
        let lines = parse_lyrics(body);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].words.is_empty());
    }

    #[test]
    fn garbage_is_empty() {
        assert!(parse_lyrics("nope").is_empty());
        assert!(parse_lyrics(r#"{"content":[],"lrc":""}"#).is_empty());
        assert!(parse_lyrics(r#"{"error":"Forbidden"}"#).is_empty());
    }

    #[test]
    fn punctuation_parts_attach_without_a_space() {
        let body = r#"{"content":[{"timestamp":1000,"endtime":3000,"text":[
            {"text":"Hi","timestamp":1000,"endtime":1500},
            {"text":",","timestamp":1500,"endtime":1600},
            {"text":"you","timestamp":1600,"endtime":2000}]}]}"#;
        let lines = parse_lyrics(body);
        assert_eq!(lines[0].text, "Hi, you");
    }
}
