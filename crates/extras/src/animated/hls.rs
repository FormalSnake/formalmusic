//! The HLS side of an animated cover. The master playlist lists renditions;
//! the one we take is a BYTERANGE playlist over a single progressive mp4
//! named by `#EXT-X-MAP`, and that mp4 is the download.

use reqwest::Url;

/// Widest rendition worth decoding for a cover.
const MAX_WIDTH: u32 = 768;

fn attr<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let start = line.find(name)? + name.len();
    let rest = &line[start..];
    Some(
        &rest[..rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len())],
    )
}

/// The highest `AVERAGE-BANDWIDTH` avc1 rendition at most 768 px wide, as the
/// (possibly relative) path on the line after its `#EXT-X-STREAM-INF`. hvc1
/// is skipped for decoder compatibility.
pub(crate) fn pick_variant(master: &str) -> Option<&str> {
    let lines: Vec<&str> = master.lines().map(str::trim).collect();
    let mut best: Option<(u64, &str)> = None;
    for (i, line) in lines.iter().enumerate() {
        if !line.starts_with("#EXT-X-STREAM-INF:") || !line.contains("avc1") {
            continue;
        }
        let width = attr(line, "RESOLUTION=").and_then(|w| w.parse::<u32>().ok());
        if width.is_none_or(|w| w > MAX_WIDTH) {
            continue;
        }
        let Some(uri) = lines[i + 1..]
            .iter()
            .find(|l| !l.is_empty() && !l.starts_with('#'))
        else {
            continue;
        };
        let bandwidth = attr(line, "AVERAGE-BANDWIDTH=")
            .and_then(|b| b.parse().ok())
            .unwrap_or(0);
        if best.is_none_or(|(top, _)| bandwidth > top) {
            best = Some((bandwidth, uri));
        }
    }
    best.map(|(_, uri)| uri)
}

/// `reference` against `base`, which may already be absolute.
pub(crate) fn resolve(base: &str, reference: &str) -> Option<Url> {
    Url::parse(base).ok()?.join(reference).ok()
}

/// The `URI` of the rendition playlist's `#EXT-X-MAP`.
pub(crate) fn map_uri(rendition: &str) -> Option<&str> {
    let line = rendition.lines().find(|l| l.starts_with("#EXT-X-MAP:"))?;
    let start = line.find("URI=\"")? + 5;
    let end = line[start..].find('"')?;
    Some(&line[start..start + end]).filter(|uri| !uri.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = include_str!("../../fixtures/hls_master.m3u8");
    const MEDIA: &str = include_str!("../../fixtures/hls_media.m3u8");

    #[test]
    fn picks_the_best_avc1_rendition_up_to_768() {
        let variant = pick_variant(MASTER).unwrap();
        assert!(
            variant.ends_with("P1189220687_Anull_video_gr240_sdr_768x768.m3u8"),
            "{variant}"
        );
    }

    #[test]
    fn hvc1_and_iframe_streams_are_skipped() {
        let master = "#EXTM3U\n\
            #EXT-X-I-FRAME-STREAM-INF:AVERAGE-BANDWIDTH=999999,CODECS=\"avc1.64001f\",RESOLUTION=486x486,URI=\"iframe.m3u8\"\n\
            #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=900000,CODECS=\"hvc1.2.4\",RESOLUTION=486x486\nhevc.m3u8\n\
            #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=100,CODECS=\"avc1.64001f\",RESOLUTION=360x360\n\n# comment\nlow.m3u8\n";
        assert_eq!(pick_variant(master), Some("low.m3u8"));
    }

    #[test]
    fn nothing_eligible_is_none() {
        let master = "#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=5,CODECS=\"avc1\",RESOLUTION=1920x1080\nbig.m3u8\n";
        assert_eq!(pick_variant(master), None);
        assert_eq!(pick_variant(""), None);
        assert_eq!(
            pick_variant("#EXT-X-STREAM-INF:CODECS=\"avc1\",RESOLUTION=360x360\n"),
            None
        );
    }

    #[test]
    fn missing_average_bandwidth_counts_as_zero() {
        let master = "#EXT-X-STREAM-INF:CODECS=\"avc1\",RESOLUTION=360x360\na.m3u8\n\
            #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=1,CODECS=\"avc1\",RESOLUTION=360x360\nb.m3u8\n";
        assert_eq!(pick_variant(master), Some("b.m3u8"));
    }

    #[test]
    fn map_uri_names_the_mp4() {
        assert_eq!(
            map_uri(MEDIA),
            Some("P1189220687_Anull_video_gr240_sdr_768x768-.mp4")
        );
        assert_eq!(map_uri("#EXTM3U\n"), None);
    }

    #[test]
    fn relative_and_absolute_references_resolve() {
        let variant = "https://mvod.itunes.apple.com/itunes-assets/x/y_768x768.m3u8";
        assert_eq!(
            resolve(variant, "y_768x768-.mp4").unwrap().as_str(),
            "https://mvod.itunes.apple.com/itunes-assets/x/y_768x768-.mp4"
        );
        assert_eq!(
            resolve(variant, "https://cdn.example/a.mp4")
                .unwrap()
                .as_str(),
            "https://cdn.example/a.mp4"
        );
    }
}
