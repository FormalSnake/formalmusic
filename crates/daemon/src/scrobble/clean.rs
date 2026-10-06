//! Song titles and artists out of YouTube video titles and channel names.
//! Album tracks already come clean from YouTube Music; videos and uploads
//! carry "(Official Video)" tags, "Artist - Title" titles and channel names
//! such as "TheWeekndVEVO" or "Billie Eilish - Topic".

/// Words that mark a bracketed group or a `|` suffix as YouTube decoration
/// rather than part of the song's name.
const NOISE: &[&str] = &[
    "official",
    "video",
    "videoclip",
    "audio",
    "lyric",
    "lyrics",
    "visualizer",
    "visualiser",
    "mv",
    "m/v",
    "oficial",
    "officiel",
    "upgrade",
];

/// Quality tags, decoration only when nothing else is in the group: "(HD)"
/// goes, "(4K Remaster)" stays.
const QUALITY: &[&str] = &["4k", "hd", "hq", "1080p", "60fps"];

const BRACKETS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('【', '】'), ('「', '」')];

/// What separates the artist from the title in an upload's name: a hyphen,
/// an en dash or an em dash, spaced.
const SEPARATORS: &[&str] = &[" - ", " \u{2013} ", " \u{2014} ", " -- "];

/// The title without YouTube decoration, with a trailing "ft. X" written
/// the way album tracks write it.
pub fn clean_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    'outer: while !rest.is_empty() {
        for &(open, close) in BRACKETS {
            if let Some(body) = rest.strip_prefix(open)
                && let Some(end) = body.find(close)
            {
                let inner = &body[..end];
                if !is_noise(inner) {
                    out.push(open);
                    out.push_str(inner);
                    out.push(close);
                }
                rest = &body[end + close.len_utf8()..];
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    let mut title = squash(&out);
    if let Some((head, tail)) = title.split_once(" | ")
        && is_noise(tail)
    {
        title = head.trim().to_owned();
    }
    for marker in [" ft. ", " feat. ", " ft ", " featuring "] {
        let lower = title.to_lowercase();
        if let Some(at) = lower.find(marker)
            && !title[..at].contains('(')
        {
            let guest = title[at + marker.len()..].trim().to_owned();
            title = format!("{} (feat. {guest})", title[..at].trim());
            break;
        }
    }
    title
        .trim_matches(|c: char| c.is_whitespace() || c == '-' || c == '|')
        .to_owned()
}

/// A channel name as an artist name.
pub fn clean_artist(name: &str) -> String {
    let name = name.trim();
    let name = name.strip_suffix(" - Topic").unwrap_or(name);
    let name = name.strip_suffix("VEVO").unwrap_or(name);
    let name = name.strip_suffix(" Official").unwrap_or(name);
    name.trim().to_owned()
}

/// Splits an upload's "Artist - Title". The left side is the artist list;
/// when it starts with the byline's artist, that one comes first and the
/// rest are split off it.
pub fn split_upload(title: &str, byline: Option<&str>) -> Option<(Vec<String>, String)> {
    let (left, right) = SEPARATORS
        .iter()
        .filter_map(|sep| title.split_once(sep))
        .min_by_key(|(left, _)| left.len())?;
    let (left, right) = (left.trim(), right.trim());
    if left.is_empty() || right.is_empty() {
        return None;
    }
    let lead = byline
        .map(clean_artist)
        .filter(|b| !b.is_empty() && left.len() >= b.len())
        .and_then(|b| {
            let head = left.get(..b.len())?;
            (norm(head) == norm(&b)).then_some(head)
        });
    let artists = match lead {
        Some(head) => {
            let rest = left[head.len()..].trim_start();
            let rest = [",", "&", "x ", "X ", "feat.", "ft.", "and "]
                .iter()
                .find_map(|joiner| rest.strip_prefix(joiner))
                .unwrap_or(rest);
            let mut artists = vec![head.trim().to_owned()];
            artists.extend(
                rest.split([',', '&'])
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned),
            );
            artists
        }
        None => vec![left.to_owned()],
    };
    Some((artists, right.to_owned()))
}

/// Lowercase letters and digits only, for comparing names across services.
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_noise(text: &str) -> bool {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '/'))
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().any(|w| NOISE.contains(w))
        || (!words.is_empty() && words.iter().all(|w| QUALITY.contains(w)))
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real titles as YouTube shows them, with the byline YouTube Music gave.
    const UPLOADS: &[(&str, Option<&str>, &[&str], &str)] = &[
        (
            "Rick Astley - Never Gonna Give You Up (Official Music Video)",
            Some("Rick Astley"),
            &["Rick Astley"],
            "Never Gonna Give You Up",
        ),
        (
            "The Weeknd - Blinding Lights (Official Video)",
            Some("TheWeekndVEVO"),
            &["The Weeknd"],
            "Blinding Lights",
        ),
        (
            "Måneskin - Beggin' (Lyrics)",
            Some("7clouds"),
            &["Måneskin"],
            "Beggin'",
        ),
        (
            "Linkin Park - Numb [Official Music Video] [4K UPGRADE]",
            Some("Linkin Park"),
            &["Linkin Park"],
            "Numb",
        ),
        (
            "Gotye - Somebody That I Used To Know (feat. Kimbra) [Official Music Video]",
            Some("gotye"),
            &["Gotye"],
            "Somebody That I Used To Know (feat. Kimbra)",
        ),
        (
            "Queen \u{2013} Bohemian Rhapsody (Official Video Remastered)",
            Some("Queen Official"),
            &["Queen"],
            "Bohemian Rhapsody",
        ),
        (
            "Lady Gaga, Bruno Mars - Die With A Smile (Official Music Video)",
            Some("Lady Gaga"),
            &["Lady Gaga", "Bruno Mars"],
            "Die With A Smile",
        ),
        (
            "Daft Punk - Get Lucky (Official Audio) ft. Pharrell Williams, Nile Rodgers",
            Some("Daft Punk"),
            &["Daft Punk"],
            "Get Lucky (feat. Pharrell Williams, Nile Rodgers)",
        ),
        (
            "Rammstein - Du Hast (Official 4K Video)",
            Some("Rammstein Official"),
            &["Rammstein"],
            "Du Hast",
        ),
        (
            "Hozier - Too Sweet (Official Visualiser)",
            Some("HozierVEVO"),
            &["Hozier"],
            "Too Sweet",
        ),
        (
            "Avicii - The Nights (Lyric Video)",
            Some("Avicii"),
            &["Avicii"],
            "The Nights",
        ),
        (
            "Imagine Dragons - Believer | Official Audio",
            Some("ImagineDragonsVEVO"),
            &["Imagine Dragons"],
            "Believer",
        ),
    ];

    #[test]
    fn cleans_real_upload_titles() {
        for &(raw, byline, artists, title) in UPLOADS {
            let cleaned = clean_title(raw);
            let (got_artists, got_title) =
                split_upload(&cleaned, byline).unwrap_or_else(|| panic!("no split: {raw}"));
            assert_eq!(got_title, title, "{raw}");
            assert_eq!(got_artists, artists, "{raw}");
        }
    }

    #[test]
    fn keeps_meaningful_brackets() {
        for title in [
            "Bohemian Rhapsody (Live Aid)",
            "Blinding Lights (Chromatics Remix)",
            "My Destiny - Slowed",
            "Never Gonna Give You Up (2022 Remaster)",
            "Mr. Brightside (Radio Edit)",
        ] {
            assert_eq!(clean_title(title), title);
        }
    }

    #[test]
    fn strips_tags_from_video_titles() {
        assert_eq!(
            clean_title("Never Gonna Give You Up (Official Video) (4K Remaster)"),
            "Never Gonna Give You Up (4K Remaster)"
        );
        assert_eq!(clean_title("Anti-Hero (Official Lyric Video)"), "Anti-Hero");
        assert_eq!(clean_title("TQG [Official Video]"), "TQG");
        assert_eq!(clean_title("Without Me   (HD)"), "Without Me");
    }

    #[test]
    fn channels_become_artists() {
        assert_eq!(clean_artist("Billie Eilish - Topic"), "Billie Eilish");
        assert_eq!(clean_artist("EminemVEVO"), "Eminem");
        assert_eq!(clean_artist("Queen Official"), "Queen");
        assert_eq!(clean_artist("Daft Punk"), "Daft Punk");
    }

    #[test]
    fn titles_without_a_dash_do_not_split() {
        assert_eq!(split_upload("Blinding Lights", Some("The Weeknd")), None);
    }
}
