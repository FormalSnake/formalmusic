//! LRC parsing: `[mm:ss]`, `[mm:ss.xx]` and `[mm:ss.xxx]` line stamps, and the
//! enhanced format's inline `<mm:ss.xx>` word stamps.
//!
//! A line may carry several leading stamps (one entry per stamp). A bracket
//! whose contents are not a bare `minutes:seconds` pair (`[ar:..]`, `[ti:..]`,
//! `[offset:..]`) is metadata and skipped. Two entries on the same time merge:
//! the first one's text is kept and the second is folded in below it in
//! parentheses, which is how translations are usually published.

use formalmusic_api::{LyricLine, LyricWord};

/// A word with no known end runs this long.
const WORD_FALLBACK_MS: u64 = 350;

struct Entry {
    start_ms: u64,
    text: String,
    words: Vec<LyricWord>,
}

/// `digits ":" digits ["." digits]` in milliseconds.
fn time_ms(tag: &str) -> Option<u64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let whole = seconds.split_once('.').map_or(seconds, |(w, _)| w);
    let fraction = seconds.split_once('.').map(|(_, f)| f);
    if !digits(minutes) || !digits(whole) || fraction.is_some_and(|f| !digits(f)) {
        return None;
    }
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: f64 = seconds.parse().ok()?;
    Some(minutes * 60_000 + (seconds * 1000.0).round() as u64)
}

/// Every leading `[..]` group in order, and the text after the last.
fn leading_tags(line: &str) -> (Vec<&str>, &str) {
    let mut tags = Vec::new();
    let mut rest = line;
    while let Some(inner) = rest.strip_prefix('[') {
        let Some(end) = inner.find(']') else { break };
        tags.push(&inner[..end]);
        rest = &inner[end + 1..];
    }
    (tags, rest)
}

/// Splits `rest` at its `<mm:ss.xx>` stamps. Each stamp owns the raw text up
/// to the next one, untrimmed, so a syllable split inside a word
/// (`<0:01.0>Hel<0:01.2>lo`) stays two chunks. A chunk joins the next only
/// when neither side has whitespace at the seam. Returns the words and the
/// whole span's text.
fn parse_words(rest: &str) -> (Vec<LyricWord>, String) {
    let mut stamps: Vec<(u64, usize, usize)> = Vec::new();
    let mut from = 0;
    while let Some(open) = rest[from..].find('<').map(|i| i + from) {
        let parsed = rest[open + 1..].find('>').and_then(|close| {
            Some((
                time_ms(&rest[open + 1..open + 1 + close])?,
                open + close + 2,
            ))
        });
        match parsed {
            Some((time, end)) => {
                stamps.push((time, open, end));
                from = end;
            }
            None => from = open + 1,
        }
    }
    if stamps.is_empty() {
        return (Vec::new(), String::new());
    }

    let raw: Vec<(u64, &str)> = stamps
        .iter()
        .enumerate()
        .map(|(i, &(time, _, end))| {
            let stop = stamps.get(i + 1).map_or(rest.len(), |next| next.1);
            (time, &rest[end..stop])
        })
        .collect();

    let mut words = Vec::new();
    let mut joined = String::new();
    for (i, &(time, text)) in raw.iter().enumerate() {
        joined.push_str(text);
        if text.trim().is_empty() {
            continue;
        }
        let joins_next = raw.get(i + 1).is_some_and(|&(_, next)| {
            !next.trim().is_empty()
                && !text.ends_with(char::is_whitespace)
                && !next.starts_with(char::is_whitespace)
        });
        words.push(LyricWord {
            start_ms: time,
            end_ms: time,
            text: text.trim().to_owned(),
            joins_next,
        });
    }
    (words, joined.trim().to_owned())
}

fn append_translation(existing: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let wrapped = if text.starts_with('(') && text.ends_with(')') {
        text.to_owned()
    } else {
        format!("({text})")
    };
    if !existing.is_empty() {
        existing.push('\n');
    }
    existing.push_str(&wrapped);
}

/// Lines sorted by time with equal times merged. Each line ends where the next
/// begins; the last has no end.
pub(crate) fn parse(text: &str) -> Vec<LyricLine> {
    let mut entries = Vec::new();
    for raw in text.split(['\r', '\n']) {
        let (tags, rest) = leading_tags(raw);
        let times: Vec<u64> = tags.iter().filter_map(|t| time_ms(t)).collect();
        if times.is_empty() {
            continue;
        }
        let (words, joined) = parse_words(rest);
        let line_text = if words.is_empty() {
            rest.trim().to_owned()
        } else {
            joined
        };
        for start_ms in times {
            entries.push(Entry {
                start_ms,
                text: line_text.clone(),
                words: words.clone(),
            });
        }
    }
    entries.sort_by_key(|e| e.start_ms);

    let mut lines: Vec<LyricLine> = Vec::new();
    for entry in entries {
        match lines.last_mut() {
            Some(last) if last.start_ms == entry.start_ms => {
                if last.words.is_empty() && !entry.words.is_empty() {
                    last.words = entry.words;
                }
                append_translation(&mut last.text, &entry.text);
            }
            _ => lines.push(LyricLine {
                start_ms: entry.start_ms,
                text: entry.text,
                words: entry.words,
                ..LyricLine::default()
            }),
        }
    }
    close_spans(&mut lines);
    lines
}

fn close_spans(lines: &mut [LyricLine]) {
    let starts: Vec<u64> = lines.iter().map(|l| l.start_ms).collect();
    for (i, line) in lines.iter_mut().enumerate() {
        line.end_ms = starts.get(i + 1).copied();
        fill_word_ends(line);
    }
}

/// Each word runs until the next starts; the last runs to the line's end, or
/// a short fallback when the line has none.
pub(crate) fn fill_word_ends(line: &mut LyricLine) {
    let line_end = line.end_ms;
    let starts: Vec<u64> = line.words.iter().map(|w| w.start_ms).collect();
    for (i, word) in line.words.iter_mut().enumerate() {
        word.end_ms = starts
            .get(i + 1)
            .copied()
            .or(line_end.filter(|end| *end > word.start_ms))
            .unwrap_or(word.start_ms + WORD_FALLBACK_MS);
    }
}

/// One line is usable on its own; several need a genuine increase somewhere,
/// not a run of entries stuck on one time.
pub(crate) fn has_usable_timing(lines: &[LyricLine]) -> bool {
    match lines {
        [] => false,
        [_] => true,
        _ => lines.windows(2).any(|w| w[1].start_ms > w[0].start_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[LyricLine]) -> Vec<(&str, u64)> {
        lines
            .iter()
            .map(|l| (l.text.as_str(), l.start_ms))
            .collect()
    }

    #[test]
    fn parses_stamp_precisions() {
        let lines = parse("[00:05] a\n[00:10.5] b\n[01:02.345] c\n");
        assert_eq!(texts(&lines), [("a", 5_000), ("b", 10_500), ("c", 62_345)]);
        assert_eq!(lines[0].end_ms, Some(10_500));
        assert_eq!(lines[2].end_ms, None);
    }

    #[test]
    fn skips_metadata_and_untimed_lines() {
        let lines = parse("[ar:Someone]\n[ti:Title]\n[offset:+200]\nplain line\n[00:01.00]real\n");
        assert_eq!(texts(&lines), [("real", 1_000)]);
    }

    #[test]
    fn several_stamps_yield_one_entry_each_and_sort() {
        let lines = parse("[00:20.00][00:10.00]chorus\n[00:15.00]verse\r\n");
        assert_eq!(
            texts(&lines),
            [("chorus", 10_000), ("verse", 15_000), ("chorus", 20_000)]
        );
    }

    #[test]
    fn equal_times_merge_as_parenthesised_translation() {
        let lines = parse("[00:01.00]Hello\n[00:01.00]Hola\n[00:01.00](Bonjour)\n[00:02.00]x");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello\n(Hola)\n(Bonjour)");
    }

    #[test]
    fn merge_keeps_words_from_whichever_line_has_them() {
        let lines = parse("[00:01.00]Plain\n[00:01.00]<00:01.00>Pl<00:01.20>ain\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].words.len(), 2);
    }

    #[test]
    fn enhanced_words_and_syllable_joins() {
        let lines = parse("[00:01.00]<00:01.00>Hel<00:01.20>lo <00:01.60>world\n[00:03.00]next");
        let words = &lines[0].words;
        assert_eq!(lines[0].text, "Hello world");
        let got: Vec<_> = words
            .iter()
            .map(|w| (w.text.as_str(), w.start_ms, w.end_ms, w.joins_next))
            .collect();
        assert_eq!(
            got,
            [
                ("Hel", 1_000, 1_200, true),
                ("lo", 1_200, 1_600, false),
                ("world", 1_600, 3_000, false)
            ]
        );
    }

    #[test]
    fn stray_stamp_without_text_breaks_the_join() {
        let lines = parse("[00:01.00]<00:01.00>a<00:01.10><00:01.20>b");
        assert!(lines[0].words.iter().all(|w| !w.joins_next));
        assert_eq!(lines[0].words.len(), 2);
    }

    #[test]
    fn last_word_without_line_end_gets_fallback() {
        let lines = parse("[00:01.00]<00:01.00>a <00:01.50>b");
        assert_eq!(lines[0].words[1].end_ms, 1_850);
    }

    #[test]
    fn non_time_angle_brackets_stay_text() {
        let lines = parse("[00:01.00]a <3 b");
        assert_eq!(lines[0].text, "a <3 b");
        assert!(lines[0].words.is_empty());
    }

    #[test]
    fn usable_timing_needs_an_increase() {
        assert!(!has_usable_timing(&[]));
        assert!(has_usable_timing(&parse("[00:01.00]a")));
        let stuck = |start_ms| LyricLine {
            start_ms,
            ..LyricLine::default()
        };
        assert!(!has_usable_timing(&[stuck(0), stuck(0)]));
        assert!(has_usable_timing(&parse("[00:00.00]a\n[00:01.00]b")));
    }
}
