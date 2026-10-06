//! Response parsers, one module per renderer family. They read
//! `serde_json::Value` by index, which yields `Null` for anything missing, so
//! a renamed field degrades to an absent value instead of a failed page. Only
//! a page whose core structure is gone becomes [`ApiError::Parse`].

pub mod account;
pub mod chips;
pub mod headers;
pub mod items;
pub mod lyrics;
pub mod next;
pub mod page;
pub mod player;
pub mod search;
pub mod shelves;
pub mod suggestions;

use formalmusic_api::{
    ApiError, BrowseTarget, Continuation, LibraryTab, Link, Thumbnail, Thumbnails,
};
use serde_json::Value;

pub(crate) fn missing(path: &str) -> ApiError {
    ApiError::Parse(format!("missing {path}"))
}

/// The text of a `{runs: [...]}` or `{simpleText}` object, `None` when empty.
pub fn text(v: &Value) -> Option<String> {
    if let Some(s) = v["simpleText"].as_str() {
        return non_empty(s.to_owned());
    }
    if let Some(s) = v["content"].as_str() {
        return non_empty(s.to_owned());
    }
    let runs = v["runs"].as_array()?;
    non_empty(runs.iter().filter_map(|r| r["text"].as_str()).collect())
}

fn non_empty(s: String) -> Option<String> {
    (!s.trim().is_empty()).then_some(s)
}

pub fn runs(v: &Value) -> &[Value] {
    v["runs"].as_array().map(Vec::as_slice).unwrap_or_default()
}

/// Runs split on the " • " separators YouTube puts between subtitle parts.
pub fn run_groups(runs: &[Value]) -> Vec<Vec<&Value>> {
    let mut groups = vec![Vec::new()];
    for run in runs {
        if run["text"].as_str().is_some_and(|t| t.trim() == "•") {
            groups.push(Vec::new());
        } else if let Some(last) = groups.last_mut() {
            last.push(run);
        }
    }
    groups.retain(|g| !g.is_empty());
    groups
}

pub fn group_text(group: &[&Value]) -> String {
    group
        .iter()
        .filter_map(|r| r["text"].as_str())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Every thumbnail size under `v`, smallest first. Accepts the renderer
/// wrappers YouTube nests thumbnails in.
pub fn thumbnails(v: &Value) -> Thumbnails {
    let list = [
        &v["thumbnails"],
        &v["thumbnail"]["thumbnails"],
        &v["musicThumbnailRenderer"]["thumbnail"]["thumbnails"],
        &v["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]["thumbnails"],
        &v["croppedSquareThumbnailRenderer"]["thumbnail"]["thumbnails"],
        &v["thumbnail"]["croppedSquareThumbnailRenderer"]["thumbnail"]["thumbnails"],
        &v["thumbnailRenderer"]["musicThumbnailRenderer"]["thumbnail"]["thumbnails"],
        &v["thumbnailRenderer"]["croppedSquareThumbnailRenderer"]["thumbnail"]["thumbnails"],
        &v["sources"],
        &v["image"]["sources"],
    ]
    .into_iter()
    .find_map(Value::as_array);
    let mut out: Thumbnails = list
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let url = t["url"].as_str()?;
            let url = if url.starts_with("//") {
                format!("https:{url}")
            } else {
                url.to_owned()
            };
            Some(Thumbnail {
                url,
                width: t["width"].as_u64().unwrap_or(0) as u32,
                height: t["height"].as_u64().unwrap_or(0) as u32,
            })
        })
        .collect();
    out.sort_by_key(|t| t.width);
    out
}

/// Where a `browseEndpoint` leads, as a [`BrowseTarget`].
pub fn browse_target(endpoint: &Value) -> Option<BrowseTarget> {
    let browse = &endpoint["browseEndpoint"];
    let id = browse["browseId"].as_str()?;
    let params = browse["params"].as_str().map(decode_percent);
    Some(target_for(id, params))
}

pub fn target_for(id: &str, params: Option<String>) -> BrowseTarget {
    let params_or_raw = |make: fn(String) -> BrowseTarget| match &params {
        Some(p) => make(p.clone()),
        None => BrowseTarget::Raw {
            browse_id: id.to_owned(),
            params: None,
        },
    };
    match id {
        "FEmusic_home" => match params {
            Some(params) => BrowseTarget::HomeChip { params },
            None => BrowseTarget::Home,
        },
        "FEmusic_explore" => BrowseTarget::Explore,
        "FEmusic_new_releases" => BrowseTarget::NewReleases,
        "FEmusic_charts" => BrowseTarget::Charts,
        "FEmusic_moods_and_genres" => BrowseTarget::MoodsAndGenres,
        "FEmusic_moods_and_genres_category" => {
            params_or_raw(|params| BrowseTarget::MoodCategory { params })
        }
        "FEmusic_history" => BrowseTarget::History,
        _ if library_tab(id).is_some() => BrowseTarget::Library(library_tab(id).unwrap()),
        _ if id.starts_with("MPRE") => BrowseTarget::Album(id.to_owned()),
        _ if id.starts_with("MPSP") => BrowseTarget::Podcast(id.to_owned()),
        _ if id.starts_with("MPED") => BrowseTarget::Episode(id.to_owned()),
        // Library artist rows link to MPLA plus the channel id: the artist's
        // songs in your library, which browses like an artist page.
        _ if id.starts_with("MPLA") => BrowseTarget::Artist(id.to_owned()),
        _ if id.starts_with("MPAD") => match params {
            Some(params) => BrowseTarget::ArtistShelf {
                browse_id: id.to_owned(),
                params,
            },
            None => BrowseTarget::Raw {
                browse_id: id.to_owned(),
                params: None,
            },
        },
        _ if id.starts_with("UC") => match params {
            Some(params) => BrowseTarget::ArtistShelf {
                browse_id: id.to_owned(),
                params,
            },
            None => BrowseTarget::Artist(id.to_owned()),
        },
        _ if id.starts_with("VL") => BrowseTarget::Playlist(id[2..].to_owned()),
        _ => BrowseTarget::Raw {
            browse_id: id.to_owned(),
            params,
        },
    }
}

pub(crate) const LIBRARY_TABS: [(LibraryTab, &str); 8] = [
    (LibraryTab::Playlists, "FEmusic_liked_playlists"),
    (LibraryTab::Songs, "FEmusic_liked_videos"),
    (LibraryTab::Albums, "FEmusic_liked_albums"),
    (LibraryTab::Artists, "FEmusic_library_corpus_track_artists"),
    (LibraryTab::Subscriptions, "FEmusic_library_corpus_artists"),
    (LibraryTab::Podcasts, "FEmusic_library_non_music_audio_list"),
    (
        LibraryTab::Uploads,
        "FEmusic_library_privately_owned_tracks",
    ),
    (LibraryTab::LikedSongs, "VLLM"),
];

fn library_tab(id: &str) -> Option<LibraryTab> {
    LIBRARY_TABS
        .iter()
        .find(|(_, b)| *b == id)
        .map(|(tab, _)| *tab)
}

/// Params arrive URL-encoded inside JSON (`...%3D`) and are sent back decoded.
pub fn decode_percent(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// A run as a [`Link`], following its `browseEndpoint` if it has one.
pub fn link(run: &Value) -> Option<Link> {
    let text = run["text"].as_str()?.trim();
    if text.is_empty() || is_joiner(text) {
        return None;
    }
    Some(Link {
        text: text.to_owned(),
        target: browse_target(&run["navigationEndpoint"]),
    })
}

/// The ", " and " & " runs between artist names.
pub fn is_joiner(text: &str) -> bool {
    matches!(text.trim(), "," | "&" | "•" | "" | "and" | "·")
}

/// "4:36", "1:02:03" in milliseconds.
pub fn parse_clock(text: &str) -> Option<u64> {
    let text = text.trim();
    if !text.contains(':') {
        return None;
    }
    text.split(':')
        .try_fold(0u64, |acc, part| Some(acc * 60 + part.parse::<u64>().ok()?))
        .map(|s| s * 1000)
}

/// "1 hr 3 min", "45 min", "30 sec" from podcast episodes, in milliseconds.
pub fn parse_spoken_duration(text: &str) -> Option<u64> {
    let mut total = 0;
    let mut words = text.split_whitespace();
    let mut found = false;
    while let Some(word) = words.next() {
        let Ok(n) = word.parse::<u64>() else { continue };
        let unit = words.next().unwrap_or_default();
        let secs = match unit.trim_end_matches(['s', ',']) {
            "hr" | "hour" => 3600,
            "min" | "minute" => 60,
            "sec" | "second" => 1,
            _ => continue,
        };
        total += n * secs;
        found = true;
    }
    found.then_some(total * 1000)
}

/// The token of `continuations[0]` or of a trailing `continuationItemRenderer`.
pub fn continuation(v: &Value) -> Option<Continuation> {
    let from_list = v["continuations"].as_array().and_then(|list| {
        list.iter().find_map(|c| {
            c.as_object()?
                .values()
                .find_map(|data| data["continuation"].as_str())
                .map(str::to_owned)
        })
    });
    from_list
        .or_else(|| continuation_item(v["contents"].as_array()?.last()?))
        .map(Continuation)
}

pub(crate) fn continuation_item(item: &Value) -> Option<String> {
    let renderer = &item["continuationItemRenderer"];
    renderer["continuationEndpoint"]["continuationCommand"]["token"]
        .as_str()
        .or_else(|| {
            renderer["button"]["buttonRenderer"]["command"]["continuationCommand"]["token"].as_str()
        })
        .map(str::to_owned)
}

/// The single key of a `{"somethingRenderer": {...}}` wrapper.
pub(crate) fn renderer(v: &Value) -> Option<(&str, &Value)> {
    let obj = v.as_object()?;
    obj.iter()
        .find(|(k, _)| k.ends_with("Renderer") || k.ends_with("Model"))
        .map(|(k, v)| (k.as_str(), v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_clock("4:36"), Some(276_000));
        assert_eq!(parse_clock("1:02:03"), Some(3_723_000));
        assert_eq!(parse_clock("2013"), None);
        assert_eq!(parse_spoken_duration(" • 1 hr 3 min"), Some(3_780_000));
        assert_eq!(parse_spoken_duration("45 min"), Some(2_700_000));
        assert_eq!(parse_spoken_duration("Played"), None);
    }

    #[test]
    fn targets_from_browse_ids() {
        assert_eq!(
            target_for("VLPL123", None),
            BrowseTarget::Playlist("PL123".into())
        );
        assert_eq!(
            target_for("MPREb_x", None),
            BrowseTarget::Album("MPREb_x".into())
        );
        assert_eq!(
            target_for("UCabc", None),
            BrowseTarget::Artist("UCabc".into())
        );
        assert!(matches!(
            target_for("UCabc", Some("p".into())),
            BrowseTarget::ArtistShelf { .. }
        ));
        assert_eq!(
            target_for("FEmusic_liked_albums", None),
            BrowseTarget::Library(LibraryTab::Albums)
        );
        assert_eq!(decode_percent("ggMCCAI%3D"), "ggMCCAI=");
    }
}
