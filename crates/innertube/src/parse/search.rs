//! `search` responses: a tabbed section list holding the top result card,
//! result shelves, and "did you mean" corrections.

use super::missing;
use super::shelves::sections;
use crate::Result;
use formalmusic_api::{Continuation, SearchFilter, SearchResults};
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
    let mut sections = parsed.sections;
    for section in &mut sections {
        section.continuation = section.continuation.take().map(tag);
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
