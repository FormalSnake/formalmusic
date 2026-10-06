//! `chipCloudRenderer`: the mood chips over Home.

use super::{decode_percent, text};
use formalmusic_api::Chip;
use serde_json::Value;

pub fn chips(cloud: &Value) -> Vec<Chip> {
    cloud["chips"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let c = &c["chipCloudChipRenderer"];
            let selected = c["isSelected"].as_bool().unwrap_or(false);
            let endpoint = &c["navigationEndpoint"];
            let params = endpoint["browseEndpoint"]["params"]
                .as_str()
                .or_else(|| endpoint["searchEndpoint"]["params"].as_str())?;
            Some(Chip {
                title: text(&c["text"])?,
                params: decode_percent(params),
                selected,
            })
        })
        .collect()
}
