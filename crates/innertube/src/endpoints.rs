//! Request-side constants: which `browseId` and `params` each page is, and the
//! protobuf `params` blobs behind the search filter chips.

use crate::parse::LIBRARY_TABS;
use formalmusic_api::{BrowseTarget, SearchFilter};

/// `(browseId, params)` for a target.
pub(crate) fn browse_request(target: &BrowseTarget) -> (String, Option<String>) {
    let plain = |id: &str| (id.to_owned(), None);
    match target {
        BrowseTarget::Home => plain("FEmusic_home"),
        BrowseTarget::HomeChip { params } => ("FEmusic_home".into(), Some(params.clone())),
        BrowseTarget::Explore => plain("FEmusic_explore"),
        BrowseTarget::NewReleases => plain("FEmusic_new_releases"),
        BrowseTarget::Charts => plain("FEmusic_charts"),
        BrowseTarget::MoodsAndGenres => plain("FEmusic_moods_and_genres"),
        BrowseTarget::MoodCategory { params } => (
            "FEmusic_moods_and_genres_category".into(),
            Some(params.clone()),
        ),
        BrowseTarget::Library(tab) => {
            let id = LIBRARY_TABS
                .iter()
                .find(|(t, _)| t == tab)
                .map(|(_, id)| *id)
                .unwrap_or("FEmusic_library_landing");
            plain(id)
        }
        BrowseTarget::History => plain("FEmusic_history"),
        BrowseTarget::Album(id)
        | BrowseTarget::Artist(id)
        | BrowseTarget::Podcast(id)
        | BrowseTarget::Episode(id) => plain(id),
        BrowseTarget::Playlist(id) => (playlist_browse_id(id), None),
        BrowseTarget::ArtistShelf { browse_id, params } => {
            (browse_id.clone(), Some(params.clone()))
        }
        BrowseTarget::Raw { browse_id, params } => (browse_id.clone(), params.clone()),
    }
}

pub(crate) fn playlist_browse_id(playlist_id: &str) -> String {
    if playlist_id.starts_with("VL") {
        playlist_id.to_owned()
    } else {
        format!("VL{playlist_id}")
    }
}

/// Mutation endpoints take the bare playlist id.
pub(crate) fn bare_playlist_id(playlist_id: &str) -> &str {
    playlist_id.strip_prefix("VL").unwrap_or(playlist_id)
}

/// The `params` for each filter chip, as ytmusicapi sends them. The web app's
/// own chips differ after the filter byte (shelf order), and both work.
pub(crate) fn search_params(filter: SearchFilter) -> &'static str {
    match filter {
        SearchFilter::Songs => "EgWKAQIIAWoMEA4QChADEAQQCRAF",
        SearchFilter::Videos => "EgWKAQIQAWoMEA4QChADEAQQCRAF",
        SearchFilter::Albums => "EgWKAQIYAWoMEA4QChADEAQQCRAF",
        SearchFilter::Artists => "EgWKAQIgAWoMEA4QChADEAQQCRAF",
        SearchFilter::CommunityPlaylists => "EgeKAQQoAEABagwQDhAKEAMQBBAJEAU=",
        SearchFilter::FeaturedPlaylists => "EgeKAQQoADgBagwQDhAKEAMQBBAJEAU=",
        SearchFilter::Podcasts => "EgWKAQJQAWoMEA4QChADEAQQCRAF",
        SearchFilter::Episodes => "EgWKAQJIAWoMEA4QChADEAQQCRAF",
        SearchFilter::Profiles => "EgWKAQJYAWoMEA4QChADEAQQCRAF",
        SearchFilter::Library => "agIYBA==",
    }
}
