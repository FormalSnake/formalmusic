//! `musicNavigationButtonRenderer`: mood and genre tiles.

use crate::parse::{decode_percent, text};
use formalmusic_api::Item;
use serde_json::Value;

/// Only mood and genre tiles become items. The Explore page's own buttons
/// (New releases, Charts, Moods & genres) link to fixed pages the client
/// already knows.
pub fn navigation_button(r: &Value) -> Option<Item> {
    let endpoint = &r["clickCommand"]["browseEndpoint"];
    if endpoint["browseId"].as_str() != Some("FEmusic_moods_and_genres_category") {
        return None;
    }
    Some(Item::Mood {
        title: text(&r["buttonText"])?,
        params: decode_percent(endpoint["params"].as_str()?),
        color: r["solid"]["leftStripeColor"].as_u64().map(|c| c as u32),
    })
}
