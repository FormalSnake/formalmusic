//! `search` responses: a tabbed section list holding the top result card,
//! result shelves, and "did you mean" corrections.

use super::missing;
use super::shelves::sections;
use crate::Result;
use formalmusic_api::{
    Continuation, Item, SearchFilter, SearchResults, Section, SectionLayout, TrackKind,
};
use serde_json::Value;

/// Search continuations must go back to the `search` endpoint, while the
/// [`Continuation`] the client holds is opaque. Tokens get this prefix so
/// [`crate::Client::continuation`] knows where to send them.
pub(crate) const CONTINUATION_PREFIX: &str = "search:";

pub(crate) fn tag(continuation: Continuation) -> Continuation {
    Continuation(format!("{CONTINUATION_PREFIX}{}", continuation.0))
}

pub fn parse_search(
    query: &str,
    filter: Option<SearchFilter>,
    json: &Value,
) -> Result<SearchResults> {
    let contents = &json["contents"];
    let list = if let Some(tabs) = contents["tabbedSearchResultsRenderer"]["tabs"].as_array() {
        let tab = tabs
            .iter()
            .find(|t| t["tabRenderer"]["selected"].as_bool() == Some(true))
            .or(tabs.first());
        &tab.ok_or_else(|| missing("contents.tabbedSearchResultsRenderer.tabs[0]"))?["tabRenderer"]
            ["content"]["sectionListRenderer"]
    } else if contents["sectionListRenderer"].is_object() {
        &contents["sectionListRenderer"]
    } else {
        return Err(missing("contents.tabbedSearchResultsRenderer"));
    };
    let parsed = sections(&list["contents"]);
    let mut sections = match filter {
        Some(_) => parsed.sections,
        None => by_category(parsed.sections),
    };
    for section in &mut sections {
        section.continuation = section.continuation.take().map(tag);
        // Some rows under the Videos filter carry no music video type and
        // no "Video" label, which would make them songs.
        if filter == Some(SearchFilter::Videos) {
            for item in &mut section.items {
                if let Item::Track(track) = item {
                    track.kind = TrackKind::Video;
                }
            }
        }
    }
    // A filtered search is one long shelf; its continuation is the page's.
    let continuation = match filter {
        Some(_) => sections.iter().rev().find_map(|s| s.continuation.clone()),
        None => None,
    };
    Ok(SearchResults {
        query: query.to_owned(),
        filter,
        correction: parsed.correction,
        sections,
        continuation,
    })
}

/// The All tab's shelves in the web app's order: title, the filter its
/// "Show all" opens, and how it lays out.
const CATEGORIES: [(&str, SearchFilter, SectionLayout); 9] = [
    ("Songs", SearchFilter::Songs, SectionLayout::List),
    ("Videos", SearchFilter::Videos, SectionLayout::List),
    ("Albums", SearchFilter::Albums, SectionLayout::Carousel),
    ("Artists", SearchFilter::Artists, SectionLayout::Carousel),
    (
        "Community playlists",
        SearchFilter::CommunityPlaylists,
        SectionLayout::Carousel,
    ),
    (
        "Featured playlists",
        SearchFilter::FeaturedPlaylists,
        SectionLayout::Carousel,
    ),
    ("Episodes", SearchFilter::Episodes, SectionLayout::List),
    ("Profiles", SearchFilter::Profiles, SectionLayout::Carousel),
    ("Podcasts", SearchFilter::Podcasts, SectionLayout::Carousel),
];

/// YouTube sends the All tab as the top result card and then one untitled
/// row per result, every kind interleaved. Those rows are sorted into one
/// titled shelf per kind, keeping YouTube's order within each. Titled
/// shelves, as YouTube sent them before, stay as they are and get the
/// filter their title names.
fn by_category(parsed: Vec<Section>) -> Vec<Section> {
    let mut out = Vec::new();
    let mut buckets: Vec<Vec<Item>> = vec![Vec::new(); CATEGORIES.len()];
    for mut section in parsed {
        if section.title.is_some() || section.layout != SectionLayout::List {
            section.filter = CATEGORIES
                .iter()
                .find(|(title, ..)| section.title.as_deref() == Some(title))
                .map(|(_, filter, _)| *filter);
            out.push(section);
            continue;
        }
        for item in section.items {
            match category(&item)
                .and_then(|filter| CATEGORIES.iter().position(|(_, f, _)| *f == filter))
            {
                Some(n) => buckets[n].push(item),
                None => tracing::debug!(?item, "search result of no filter's kind"),
            }
        }
    }
    out.extend(
        CATEGORIES
            .iter()
            .zip(buckets)
            .filter(|(_, items)| !items.is_empty())
            .map(|((title, filter, layout), items)| Section {
                title: Some((*title).to_owned()),
                layout: *layout,
                items,
                filter: Some(*filter),
                ..Section::default()
            }),
    );
    out
}

/// The filter whose results `item` would be among.
fn category(item: &Item) -> Option<SearchFilter> {
    Some(match item {
        Item::Track(track) => match track.kind {
            TrackKind::Song | TrackKind::Upload => SearchFilter::Songs,
            TrackKind::Video => SearchFilter::Videos,
            TrackKind::Episode => SearchFilter::Episodes,
        },
        Item::Album { .. } => SearchFilter::Albums,
        // Profiles are channels too, told apart by their row's label.
        Item::Artist { subtitle, .. }
            if subtitle
                .as_deref()
                .is_some_and(|s| s.starts_with("Profile")) =>
        {
            SearchFilter::Profiles
        }
        Item::Artist { .. } => SearchFilter::Artists,
        // YouTube Music's own playlists, the Featured filter's, are its
        // RDCLAK mixes; everyone else's are community playlists.
        Item::Playlist { playlist_id, .. } if playlist_id.starts_with("RDCLAK") => {
            SearchFilter::FeaturedPlaylists
        }
        Item::Playlist { .. } => SearchFilter::CommunityPlaylists,
        Item::Podcast { .. } => SearchFilter::Podcasts,
        Item::Mood { .. } | Item::Shortcut { .. } => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A library search answers with the YouTube Music and Library tabs, the
    /// second one selected.
    #[test]
    fn a_library_search_reads_the_selected_tab() {
        let shelf = |title: &str| {
            json!({"musicShelfRenderer": {
                "title": {"runs": [{"text": title}]},
                "contents": [{"musicResponsiveListItemRenderer": {
                    "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{
                        "text": "Song",
                        "navigationEndpoint": {"watchEndpoint": {"videoId": "abc"}}
                    }]}}}],
                    "playlistItemData": {"videoId": "abc"}
                }}]
            }})
        };
        let tab = |selected: bool, title: &str| json!({"tabRenderer": {"selected": selected, "content": {"sectionListRenderer": {"contents": [shelf(title)]}}}});
        let response = json!({"contents": {"tabbedSearchResultsRenderer": {"tabs": [
            tab(false, "From YouTube Music"),
            tab(true, "From your library")
        ]}}});
        let results = parse_search("abc", Some(SearchFilter::Library), &response).unwrap();
        assert_eq!(
            results.sections[0].title.as_deref(),
            Some("From your library")
        );
    }
}
