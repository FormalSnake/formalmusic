//! The iTunes lookups that turn "artist, album" into an Apple Music album id.
//!
//! iTunes' search is a name search, not an id lookup, so it returns every
//! artist sharing the term ("Tyler, The Creator" next to "Not Tyler, The
//! Creator" and "Tyla"). Only an exact match on the normalised name counts;
//! anything else is a miss, never a guess.

use crate::{Error, Result};
use serde::Deserialize;

#[derive(Deserialize)]
struct Lookup {
    #[serde(default)]
    results: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    wrapper_type: Option<String>,
    artist_name: Option<String>,
    artist_id: Option<u64>,
    collection_name: Option<String>,
    collection_id: Option<u64>,
}

/// Folds case, punctuation and whitespace, and strips trailing "- Single" /
/// "- EP" markers and "(Deluxe)" or "[Remastered]" edition tags, in either
/// order and repeatedly: "Anti (Deluxe) - Single" and "Anti - Single (Deluxe)"
/// both become "anti". Only for comparing the two sides, never for URLs.
pub(crate) fn normalize(name: &str) -> String {
    let mut s = name.trim();
    loop {
        let stripped = strip_edition_tag(s).or_else(|| strip_release_suffix(s));
        match stripped {
            Some(rest) => s = rest,
            None => break,
        }
    }
    let mut out = String::new();
    for word in s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().flat_map(char::to_lowercase));
    }
    out
}

/// A trailing `(..)` or `[..]` that contains no brackets itself.
fn strip_edition_tag(s: &str) -> Option<&str> {
    if !(s.ends_with(')') || s.ends_with(']')) {
        return None;
    }
    let inner = &s[..s.len() - 1];
    let open = inner.rfind(['(', '['])?;
    if inner[open + 1..].contains([')', ']']) {
        return None;
    }
    Some(s[..open].trim_end())
}

fn strip_release_suffix(s: &str) -> Option<&str> {
    for suffix in ["single", "ep"] {
        let cut = s.len().checked_sub(suffix.len())?;
        if s.is_char_boundary(cut)
            && s[cut..].eq_ignore_ascii_case(suffix)
            && let Some(rest) = s[..cut].trim_end().strip_suffix('-')
        {
            return Some(rest.trim_end());
        }
    }
    None
}

fn parse(body: &str) -> Result<Vec<Item>> {
    serde_json::from_str::<Lookup>(body)
        .map(|l| l.results)
        .map_err(|_| Error::Malformed("itunes"))
}

/// The id of the artist whose normalised name equals `artist`'s.
pub(crate) fn artist_id(body: &str, artist: &str) -> Result<Option<u64>> {
    let target = normalize(artist);
    if target.is_empty() {
        return Ok(None);
    }
    Ok(parse(body)?
        .into_iter()
        .find(|item| {
            item.artist_name
                .as_deref()
                .is_some_and(|n| normalize(n) == target)
        })
        .and_then(|item| item.artist_id))
}

/// The artist lookup echoes the artist itself ahead of every album, so only
/// collections count. An exact match on the raw name wins over a normalised
/// one, so a plain "IGOR" picks the plain edition over a "(Deluxe)" reissue
/// listed earlier.
pub(crate) fn collection_id(body: &str, album: &str) -> Result<Option<u64>> {
    let target = normalize(album);
    if target.is_empty() {
        return Ok(None);
    }
    let mut normalized_match = None;
    for item in parse(body)? {
        if item.wrapper_type.as_deref() != Some("collection") {
            continue;
        }
        let Some(name) = item.collection_name.as_deref() else {
            continue;
        };
        if name == album {
            return Ok(item.collection_id);
        }
        if normalized_match.is_none() && normalize(name) == target {
            normalized_match = item.collection_id;
        }
    }
    Ok(normalized_match)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTISTS: &str = include_str!("../../fixtures/itunes_artist_search.json");
    const ALBUMS: &str = include_str!("../../fixtures/itunes_albums.json");

    #[test]
    fn normalize_folds_and_strips() {
        assert_eq!(normalize("Anti (Deluxe) - Single"), "anti");
        assert_eq!(normalize("Anti - Single (Deluxe)"), "anti");
        assert_eq!(
            normalize("Midnights [Remastered] (3am Edition)"),
            "midnights"
        );
        assert_eq!(normalize("  Tyler, The Creator "), "tyler the creator");
        assert_eq!(normalize("Hot Fuss - EP"), "hot fuss");
    }

    #[test]
    fn normalize_leaves_words_that_merely_end_like_a_suffix() {
        assert_eq!(normalize("Sleep"), "sleep");
        assert_eq!(normalize("Bullet Single"), "bullet single");
        assert_eq!(normalize("Nested (a (b))"), "nested a b");
    }

    #[test]
    fn normalize_keeps_non_latin_names_distinct() {
        assert_ne!(normalize("宇多田ヒカル"), normalize("椎名林檎"));
        assert_eq!(
            artist_id(
                r#"{"results":[{"artistName":"椎名林檎","artistId":1}]}"#,
                "宇多田ヒカル"
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn artist_must_match_exactly_after_normalising() {
        assert_eq!(artist_id(ARTISTS, "Taylor Swift").unwrap(), Some(159260351));
        assert_eq!(
            artist_id(ARTISTS, "taylor  swift").unwrap(),
            Some(159260351)
        );
        assert_eq!(artist_id(ARTISTS, "Taylor").unwrap(), Some(1144510695));
        assert_eq!(artist_id(ARTISTS, "Taylor Swif").unwrap(), None);
        assert_eq!(artist_id(ARTISTS, "").unwrap(), None);
    }

    #[test]
    fn exact_album_name_beats_normalised_matches() {
        assert_eq!(
            collection_id(ALBUMS, "Midnights").unwrap(),
            Some(1645937456)
        );
        assert_eq!(
            collection_id(ALBUMS, "Midnights (3am Edition)").unwrap(),
            Some(1650841512)
        );
    }

    #[test]
    fn edition_tags_fall_back_to_the_normalised_match() {
        assert_eq!(
            collection_id(ALBUMS, "The Life of a Showgirl (Deluxe)").unwrap(),
            Some(1838810949)
        );
    }

    #[test]
    fn unknown_album_and_artist_row_are_misses() {
        assert_eq!(collection_id(ALBUMS, "Fearless").unwrap(), None);
        assert_eq!(collection_id(ALBUMS, "Taylor Swift").unwrap(), None);
    }

    #[test]
    fn malformed_body_is_an_error_not_a_miss() {
        assert!(artist_id("<html>", "x").is_err());
        assert!(collection_id("nope", "x").is_err());
    }
}
