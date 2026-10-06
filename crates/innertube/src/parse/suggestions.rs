//! `music/get_search_suggestions`: query completions and, under them, direct
//! hits (artists, songs, albums).

use super::items::item;
use super::{renderer, text};
use formalmusic_api::Suggestion;
use serde_json::Value;

pub fn parse_suggestions(json: &Value) -> Vec<Suggestion> {
    let sections = json["contents"].as_array().into_iter().flatten();
    let entries = sections.flat_map(|s| {
        s["searchSuggestionsSectionRenderer"]["contents"]
            .as_array()
            .into_iter()
            .flatten()
    });
    entries
        .filter_map(|entry| match renderer(entry)? {
            ("searchSuggestionRenderer", r) => query(r, false),
            ("historySuggestionRenderer", r) => query(r, true),
            _ => item(entry).map(Suggestion::Item),
        })
        .collect()
}

fn query(r: &Value, from_history: bool) -> Option<Suggestion> {
    let text = r["navigationEndpoint"]["searchEndpoint"]["query"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| text(&r["suggestion"]))?;
    Some(Suggestion::Query { text, from_history })
}
