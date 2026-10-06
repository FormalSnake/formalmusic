//! music.apple.com's anonymous web-player token and the amp-api response it
//! unlocks. The token sits in the main JS bundle that any album page
//! references; it is held in memory only.

use crate::{Error, Result};
use serde::Deserialize;

/// `/assets/index~<hash>.js`, the bundle the album page loads.
pub(crate) fn asset_path(html: &str) -> Option<&str> {
    const PREFIX: &str = "/assets/index~";
    let mut from = 0;
    while let Some(i) = html[from..].find(PREFIX).map(|i| i + from) {
        let hash = &html[i + PREFIX.len()..];
        let len = hash
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(hash.len());
        if len > 0 && hash[len..].starts_with(".js") {
            return Some(&html[i..i + PREFIX.len() + len + 3]);
        }
        from = i + PREFIX.len();
    }
    None
}

/// The first quoted JWT in the bundle.
pub(crate) fn token(js: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(i) = js[from..].find("\"eyJ").map(|i| i + from + 1) {
        let rest = &js[i..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
            .unwrap_or(rest.len());
        if rest[len..].starts_with('"') {
            return Some(&rest[..len]);
        }
        from = i;
    }
    None
}

#[derive(Deserialize)]
struct Albums {
    data: Option<Vec<Album>>,
}

#[derive(Deserialize)]
struct Album {
    attributes: Option<Attributes>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attributes {
    editorial_video: Option<EditorialVideo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditorialVideo {
    motion_detail_square: Option<Motion>,
    motion_square_video1x1: Option<Motion>,
}

#[derive(Deserialize)]
struct Motion {
    video: Option<String>,
}

/// The HLS master playlist URL of the album's square motion video. `None`
/// when the album has no editorial video, which is the common case. An
/// answer without album attributes at all is an error.
pub(crate) fn editorial_video(body: &str) -> Result<Option<String>> {
    let albums: Albums = serde_json::from_str(body).map_err(|_| Error::Malformed("amp-api"))?;
    let attributes = albums
        .data
        .and_then(|d| d.into_iter().next())
        .and_then(|a| a.attributes)
        .ok_or(Error::Malformed("amp-api"))?;
    let Some(editorial) = attributes.editorial_video else {
        return Ok(None);
    };
    let video = |m: Option<Motion>| m.and_then(|m| m.video).filter(|v| !v.is_empty());
    Ok(video(editorial.motion_detail_square).or_else(|| video(editorial.motion_square_video1x1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMP: &str = include_str!("../../fixtures/amp_editorial_video.json");

    #[test]
    fn finds_the_bundle_path_in_the_album_page() {
        let html =
            r#"<script type="module" crossorigin src="/assets/index~c10ba4a68d.js"></script>"#;
        assert_eq!(asset_path(html), Some("/assets/index~c10ba4a68d.js"));
    }

    #[test]
    fn skips_lookalikes_without_a_js_extension() {
        let html = r#"<link href="/assets/index~abc.css"><script src="/assets/index~def456.js">"#;
        assert_eq!(asset_path(html), Some("/assets/index~def456.js"));
        assert_eq!(asset_path("<html></html>"), None);
    }

    #[test]
    fn finds_the_first_quoted_jwt() {
        let js = r#"var a="eyJnotquoted;var b={token:"eyJhbGciOi.J9.sig_-x"};"#;
        assert_eq!(token(js), Some("eyJhbGciOi.J9.sig_-x"));
        assert_eq!(token("no tokens here"), None);
        assert_eq!(token(r#""eyJunterminated"#), None);
    }

    #[test]
    fn editorial_video_is_the_master_playlist() {
        let url = editorial_video(AMP).unwrap().unwrap();
        assert!(
            url.starts_with("https://mvod.itunes.apple.com/")
                && url.ends_with("P1189220687_default.m3u8")
        );
    }

    #[test]
    fn square_1x1_is_the_fallback_field() {
        let body = r#"{"data":[{"attributes":{"editorialVideo":{"motionSquareVideo1x1":{"video":"https://x/y.m3u8"}}}}]}"#;
        assert_eq!(
            editorial_video(body).unwrap().as_deref(),
            Some("https://x/y.m3u8")
        );
    }

    #[test]
    fn albums_without_motion_are_none() {
        assert_eq!(
            editorial_video(r#"{"data":[{"attributes":{"name":"x"}}]}"#).unwrap(),
            None
        );
        let empty =
            r#"{"data":[{"attributes":{"editorialVideo":{"motionDetailSquare":{"video":""}}}}]}"#;
        assert_eq!(editorial_video(empty).unwrap(), None);
    }

    #[test]
    fn answers_without_attributes_are_errors() {
        assert!(editorial_video(r#"{"data":[]}"#).is_err());
        assert!(editorial_video(r#"{"errors":[]}"#).is_err());
        assert!(editorial_video("<html>").is_err());
    }
}
