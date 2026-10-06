//! `musicNavigationButtonRenderer`: mood and genre tiles, and the Explore
//! page's own buttons (New releases, Charts, Moods & genres).

use crate::parse::{browse_target, decode_percent, text};
use formalmusic_api::Item;
use serde_json::Value;

pub fn navigation_button(r: &Value) -> Option<Item> {
    let endpoint = &r["clickCommand"]["browseEndpoint"];
    let title = text(&r["buttonText"])?;
    if endpoint["browseId"].as_str() == Some("FEmusic_moods_and_genres_category") {
        return Some(Item::Mood {
            title,
            params: decode_percent(endpoint["params"].as_str()?),
            color: r["solid"]["leftStripeColor"].as_u64().map(|c| c as u32),
        });
    }
    Some(Item::Shortcut {
        title,
        target: browse_target(&r["clickCommand"])?,
        icon: r["iconStyle"]["icon"]["iconType"]
            .as_str()
            .map(str::to_owned),
    })
}
