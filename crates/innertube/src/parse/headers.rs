//! Page headers: `musicImmersiveHeaderRenderer` and `musicVisualHeaderRenderer`
//! (artists), `musicResponsiveHeaderRenderer` and the older
//! `musicDetailHeaderRenderer` (albums, playlists, podcasts),
//! `musicEditablePlaylistDetailHeaderRenderer` (playlists you own), and
//! `musicHeaderRenderer` (a plain title).

use super::{link, renderer, runs, text, thumbnails};
use formalmusic_api::{Header, Link, Privacy};
use serde_json::Value;

pub fn header(v: &Value) -> Option<Header> {
    let (key, r) = renderer(v)?;
    match key {
        "musicImmersiveHeaderRenderer" | "musicVisualHeaderRenderer" => artist(r),
        "musicResponsiveHeaderRenderer" | "musicDetailHeaderRenderer" => detail(r),
        "musicEditablePlaylistDetailHeaderRenderer" => editable(r),
        "musicHeaderRenderer" => Some(Header::Title {
            title: text(&r["title"])?,
        }),
        _ => {
            tracing::debug!(renderer = key, "skipping unknown header renderer");
            None
        }
    }
}

fn artist(r: &Value) -> Option<Header> {
    let subscribe = &r["subscriptionButton"]["subscribeButtonRenderer"];
    let watch_playlist = |button: &Value| {
        button["buttonRenderer"]["navigationEndpoint"]["watchEndpoint"]["playlistId"]
            .as_str()
            .map(str::to_owned)
    };
    let thumbs = thumbnails(&r["thumbnail"]);
    Some(Header::Artist {
        name: text(&r["title"])?,
        description: text(&r["description"]),
        thumbnails: if thumbs.is_empty() {
            thumbnails(&r["foregroundThumbnail"])
        } else {
            thumbs
        },
        channel_id: subscribe["channelId"].as_str().map(str::to_owned),
        subscribed: subscribe["subscribed"].as_bool(),
        subscribers: text(&subscribe["longSubscriberCountText"])
            .or_else(|| text(&subscribe["subscriberCountText"])),
        shuffle_playlist_id: watch_playlist(&r["playButton"]),
        radio_playlist_id: watch_playlist(&r["startRadioButton"]),
        monthly_listeners: text(&r["monthlyListenerCount"]),
    })
}

fn detail(r: &Value) -> Option<Header> {
    let buttons = r["buttons"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let playlist_id = buttons.iter().find_map(|b| {
        let endpoint = &b["musicPlayButtonRenderer"]["playNavigationEndpoint"];
        endpoint["watchPlaylistEndpoint"]["playlistId"]
            .as_str()
            .or_else(|| endpoint["watchEndpoint"]["playlistId"].as_str())
    });
    let saved = buttons
        .iter()
        .find_map(|b| b["toggleButtonRenderer"]["isToggled"].as_bool());
    let subtitle: Vec<Link> = runs(&r["straplineTextOne"])
        .iter()
        .chain(runs(&r["subtitle"]))
        .filter_map(link)
        .collect();
    let description = &r["description"];
    let description = text(&description["musicDescriptionShelfRenderer"]["description"])
        .or_else(|| text(description));
    Some(Header::Detail {
        title: text(&r["title"])?,
        subtitle,
        second_subtitle: text(&r["secondSubtitle"]),
        description,
        thumbnails: thumbnails(&r["thumbnail"]),
        playlist_id: playlist_id.map(str::to_owned),
        editable: false,
        saved,
        privacy: None,
    })
}

fn editable(r: &Value) -> Option<Header> {
    let Header::Detail {
        title,
        subtitle,
        second_subtitle,
        description,
        thumbnails,
        playlist_id,
        saved,
        ..
    } = header(&r["header"])?
    else {
        return None;
    };
    let privacy = match r["editHeader"]["musicPlaylistEditHeaderRenderer"]["privacy"].as_str() {
        Some("PUBLIC") => Some(Privacy::Public),
        Some("UNLISTED") => Some(Privacy::Unlisted),
        Some("PRIVATE") => Some(Privacy::Private),
        _ => None,
    };
    Some(Header::Detail {
        title,
        subtitle,
        second_subtitle,
        description,
        thumbnails,
        playlist_id: r["playlistId"].as_str().map(str::to_owned).or(playlist_id),
        editable: true,
        saved,
        privacy,
    })
}
