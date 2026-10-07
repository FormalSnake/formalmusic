//! Shelf renderers, each becoming one [`Section`]: `musicCarouselShelfRenderer`,
//! `musicShelfRenderer`, `musicPlaylistShelfRenderer`, `gridRenderer`,
//! `musicCardShelfRenderer`, and the `itemSectionRenderer` wrapper.

use super::items::{item, items, two_row_item};
use super::{browse_target, continuation, renderer, runs, text};
use formalmusic_api::{BrowseTarget, Item, Link, Section, SectionLayout};
use serde_json::{Value, json};

#[derive(Debug, Default)]
pub struct Sections {
    pub sections: Vec<Section>,
    /// "Showing results for" or "Did you mean", on search pages.
    pub correction: Option<String>,
}

/// A `sectionListRenderer.contents` array.
pub fn sections(contents: &Value) -> Sections {
    let mut out = Sections::default();
    // Search "All" sometimes sends each result as its own header-less
    // itemSectionRenderer; runs of those are gathered into one list section.
    let mut loose: Vec<Item> = Vec::new();
    for entry in contents.as_array().into_iter().flatten() {
        let Some((key, r)) = renderer(entry) else {
            continue;
        };
        if key == "itemSectionRenderer" {
            for inner in r["contents"].as_array().into_iter().flatten() {
                match renderer(inner) {
                    Some(("showingResultsForRenderer" | "didYouMeanRenderer", c)) => {
                        out.correction = text(&c["correctedQuery"]);
                    }
                    Some((
                        "musicResponsiveListItemRenderer" | "musicMultiRowListItemRenderer",
                        _,
                    )) => {
                        loose.extend(item(inner));
                    }
                    Some((inner_key, inner_r)) => {
                        flush(&mut loose, &mut out.sections);
                        out.sections.extend(section(inner_key, inner_r));
                    }
                    None => {}
                }
            }
            continue;
        }
        flush(&mut loose, &mut out.sections);
        out.sections.extend(section(key, r));
    }
    flush(&mut loose, &mut out.sections);
    out
}

fn flush(loose: &mut Vec<Item>, sections: &mut Vec<Section>) {
    if !loose.is_empty() {
        sections.push(empty_section(SectionLayout::List, std::mem::take(loose)));
    }
}

fn empty_section(layout: SectionLayout, items: Vec<Item>) -> Section {
    Section {
        layout,
        items,
        ..Section::default()
    }
}

/// One shelf, or `None` for renderers that carry no items (descriptions,
/// the taste builder, messages) and for shelves that came back empty.
pub fn section(key: &str, r: &Value) -> Option<Section> {
    let section = match key {
        "musicCarouselShelfRenderer" | "musicImmersiveCarouselShelfRenderer" => carousel(r),
        "musicShelfRenderer" => shelf(r),
        "musicPlaylistShelfRenderer" => Section {
            continuation: continuation(r),
            ..empty_section(SectionLayout::List, items(&r["contents"]))
        },
        "gridRenderer" => Section {
            title: text(&r["header"]["gridHeaderRenderer"]["title"]),
            continuation: continuation(r),
            ..empty_section(SectionLayout::Grid, items(&r["items"]))
        },
        "musicCardShelfRenderer" => card(r),
        _ => {
            tracing::debug!(renderer = key, "skipping shelf renderer");
            return None;
        }
    };
    (!section.items.is_empty() || section.continuation.is_some()).then_some(section)
}

fn carousel(r: &Value) -> Section {
    let header = &r["header"]["musicCarouselShelfBasicHeaderRenderer"];
    let header = if header.is_object() {
        header
    } else {
        &r["header"]["musicImmersiveCarouselShelfHeaderRenderer"]
    };
    let contents = r["contents"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let layout = match contents.first().and_then(renderer).map(|(k, _)| k) {
        Some("musicResponsiveListItemRenderer") => SectionLayout::TrackGrid,
        Some("musicNavigationButtonRenderer") => SectionLayout::Grid,
        _ => SectionLayout::Carousel,
    };
    Section {
        title: text(&header["title"]),
        strapline: text(&header["strapline"]),
        more: browse_target(&header["moreContentButton"]["buttonRenderer"]["navigationEndpoint"])
            .or_else(|| {
                runs(&header["title"])
                    .first()
                    .and_then(|run| browse_target(&run["navigationEndpoint"]))
            }),
        continuation: continuation(r),
        ..empty_section(layout, items(&r["contents"]))
    }
}

fn shelf(r: &Value) -> Section {
    Section {
        title: text(&r["title"]),
        more: browse_target(&r["bottomEndpoint"]).or_else(|| {
            runs(&r["title"])
                .first()
                .and_then(|run| browse_target(&run["navigationEndpoint"]))
        }),
        continuation: continuation(r),
        ..empty_section(SectionLayout::List, items(&r["contents"]))
    }
}

/// The search "Top result" card: one big item plus a few rows under it.
fn card(r: &Value) -> Section {
    let endpoint = runs(&r["title"])
        .first()
        .map(|run| &run["navigationEndpoint"])
        .filter(|e| e.is_object())
        .unwrap_or(&r["onTap"]);
    let as_card = json!({
        "title": r["title"],
        "subtitle": r["subtitle"],
        "navigationEndpoint": endpoint,
        "thumbnailRenderer": r["thumbnail"],
        "subtitleBadges": r["subtitleBadges"],
    });
    let mut list: Vec<Item> = two_row_item(&as_card).into_iter().collect();
    let mut rows = items(&r["contents"]);
    // Rows under an artist card are that artist's songs and leave the name out.
    if let Some(Item::Artist {
        browse_id, name, ..
    }) = list.first()
    {
        let artist = Link {
            text: name.clone(),
            target: Some(BrowseTarget::Artist(browse_id.clone())),
        };
        for row in &mut rows {
            if let Item::Track(track) = row
                && track.artists.is_empty()
            {
                track.artists.push(artist.clone());
            }
        }
    }
    list.extend(rows);
    let playlist = |icon: &str| {
        r["buttons"].as_array().into_iter().flatten().find_map(|b| {
            let b = &b["buttonRenderer"];
            let endpoint = if b["command"].is_object() {
                &b["command"]
            } else {
                &b["navigationEndpoint"]
            };
            (b["icon"]["iconType"] == icon)
                .then(|| endpoint["watchPlaylistEndpoint"]["playlistId"].as_str())
                .flatten()
                .map(str::to_owned)
        })
    };
    Section {
        title: text(&r["header"]["musicCardShelfHeaderBasicRenderer"]["title"]),
        shuffle_playlist_id: playlist("MUSIC_SHUFFLE"),
        radio_playlist_id: playlist("MIX"),
        ..empty_section(SectionLayout::Hero, list)
    }
}
