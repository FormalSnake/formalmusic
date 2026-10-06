//! Whole `browse` responses: the single-column layout (Home, Explore, artists,
//! library), the two-column layout (albums, playlists, podcasts), and their
//! continuations.

use super::chips::chips;
use super::headers::header;
use super::items::item;
use super::shelves::{section, sections};
use super::{continuation, continuation_item, missing, renderer, text};
use crate::Result;
use formalmusic_api::{
    BrowseTarget, Continuation, ContinuationPage, Header, Item, Link, Page, Section, TrackKind,
};
use serde_json::Value;

pub fn parse_page(target: BrowseTarget, json: &Value) -> Result<Page> {
    let contents = &json["contents"];
    let mut page_header = header(&json["header"]);
    let mut lead_sections = Vec::new();
    let mut about = None;

    let list = if let Some(single) = contents["singleColumnBrowseResultsRenderer"].as_object() {
        let tabs = single["tabs"]
            .as_array()
            .ok_or_else(|| missing("contents.singleColumnBrowseResultsRenderer.tabs"))?;
        let tab = tabs
            .iter()
            .find(|t| t["tabRenderer"]["selected"].as_bool() == Some(true))
            .or(tabs.first());
        &tab.ok_or_else(|| missing("contents.singleColumnBrowseResultsRenderer.tabs[0]"))?["tabRenderer"]
            ["content"]["sectionListRenderer"]
    } else if let Some(two) = contents["twoColumnBrowseResultsRenderer"].as_object() {
        let primary = &two["tabs"][0]["tabRenderer"]["content"]["sectionListRenderer"]["contents"];
        for entry in primary.as_array().into_iter().flatten() {
            if let Some(found) = header(entry) {
                page_header = Some(found);
            } else if let Some((key, r)) = renderer(entry) {
                lead_sections.extend(section(key, r));
            }
        }
        if page_header.is_none() && lead_sections.is_empty() {
            return Err(missing(
                "contents.twoColumnBrowseResultsRenderer.tabs[0]...sectionListRenderer.contents",
            ));
        }
        &two["secondaryContents"]["sectionListRenderer"]
    } else if contents["sectionListRenderer"].is_object() {
        &contents["sectionListRenderer"]
    } else {
        return Err(missing("contents.singleColumnBrowseResultsRenderer"));
    };

    for entry in list["contents"].as_array().into_iter().flatten() {
        if let Some(shelf) = entry.get("musicDescriptionShelfRenderer") {
            about = text(&shelf["description"]);
        }
    }
    let mut all_sections = lead_sections;
    all_sections.extend(sections(&list["contents"]).sections);

    if let Some(Header::Detail {
        description: description @ None,
        ..
    }) = &mut page_header
    {
        *description = about;
    }
    if let (BrowseTarget::Album(browse_id), Some(header)) = (&target, &page_header) {
        fill_album_tracks(browse_id, header, &mut all_sections);
    }

    Ok(Page {
        target,
        header: page_header,
        chips: chips(&list["header"]["chipCloudRenderer"]),
        sections: all_sections,
        continuation: continuation(list),
    })
}

/// Album rows leave out what the header already shows (album, artwork,
/// artists). Tracks travel to the queue on their own, so they get it back.
/// A row whose song has a music video links the video
/// (`MUSIC_VIDEO_TYPE_OMV`), but the album lists it as a song, and so does
/// the web app.
fn fill_album_tracks(browse_id: &str, header: &Header, sections: &mut [Section]) {
    let Header::Detail {
        title,
        subtitle,
        thumbnails,
        ..
    } = header
    else {
        return;
    };
    let album = Link {
        text: title.clone(),
        target: Some(BrowseTarget::Album(browse_id.to_owned())),
    };
    let artists: Vec<Link> = subtitle
        .iter()
        .filter(|l| matches!(l.target, Some(BrowseTarget::Artist(_))))
        .cloned()
        .collect();
    for section in sections.iter_mut().take(1) {
        for item in &mut section.items {
            if let Item::Track(track) = item {
                track.album.get_or_insert_with(|| album.clone());
                if track.thumbnails.is_empty() {
                    track.thumbnails = thumbnails.clone();
                }
                if track.artists.is_empty() {
                    track.artists = artists.clone();
                }
                if track.kind == TrackKind::Video {
                    track.kind = TrackKind::Song;
                }
            }
        }
    }
}

pub fn parse_continuation(json: &Value) -> Result<ContinuationPage> {
    let mut page = ContinuationPage {
        sections: Vec::new(),
        items: Vec::new(),
        continuation: None,
    };
    if let Some((key, c)) = json["continuationContents"]
        .as_object()
        .and_then(|o| o.iter().next())
    {
        if key == "sectionListContinuation" {
            page.sections = sections(&c["contents"]).sections;
        } else {
            let list = if c["items"].is_array() {
                &c["items"]
            } else {
                &c["contents"]
            };
            add_entries(list, &mut page);
        }
        page.continuation = continuation(c);
        return Ok(page);
    }
    let actions = json["onResponseReceivedActions"]
        .as_array()
        .ok_or_else(|| missing("continuationContents or onResponseReceivedActions"))?;
    for action in actions {
        let list = &action["appendContinuationItemsAction"]["continuationItems"];
        let list = if list.is_array() {
            list
        } else {
            &action["reloadContinuationItemsCommand"]["continuationItems"]
        };
        add_entries(list, &mut page);
        if let Some(token) = list
            .as_array()
            .and_then(|l| l.last())
            .and_then(continuation_item)
        {
            page.continuation = Some(Continuation(token));
        }
    }
    Ok(page)
}

fn add_entries(list: &Value, page: &mut ContinuationPage) {
    for entry in list.as_array().into_iter().flatten() {
        let Some((key, r)) = renderer(entry) else {
            continue;
        };
        if key == "continuationItemRenderer" {
            continue;
        }
        if let Some(found) = item(entry) {
            page.items.push(found);
        } else if let Some(found) = section(key, r) {
            page.sections.push(found);
        }
    }
}
