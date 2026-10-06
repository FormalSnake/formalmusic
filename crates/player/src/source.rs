use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Opus,
    Aac,
    /// Anything else symphonia can probe, such as a local FLAC or MP3 file.
    Other,
}

impl Codec {
    fn from_acodec(acodec: &str) -> Self {
        if acodec == "opus" {
            Codec::Opus
        } else if acodec.starts_with("mp4a") {
            Codec::Aac
        } else {
            Codec::Other
        }
    }
}

/// One playable audio stream: a googlevideo URL from yt-dlp, or a local file.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamSource {
    /// An `https://` URL, a `file://` URL or a plain local path.
    pub url: String,
    /// Container MIME type, `audio/webm` or `audio/mp4` for YouTube.
    pub mime: String,
    pub codec: Codec,
    /// Byte length, when yt-dlp reported `filesize`. Learned from the first
    /// response otherwise.
    pub content_length: Option<u64>,
    /// From the URL's `expire` parameter. Loading after this answers 403.
    pub expires_at: Option<SystemTime>,
    /// yt-dlp's `http_headers` for the format, sent on every range request.
    pub headers: Vec<(String, String)>,
    pub bitrate_kbps: Option<u32>,
}

impl StreamSource {
    pub fn file(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let (mime, codec) = match ext.as_str() {
            "webm" | "mka" | "mkv" => ("audio/webm", Codec::Opus),
            "opus" | "ogg" => ("audio/ogg", Codec::Opus),
            "m4a" | "mp4" | "aac" => ("audio/mp4", Codec::Aac),
            _ => ("application/octet-stream", Codec::Other),
        };
        Self {
            url: path.to_string_lossy().into_owned(),
            mime: mime.to_owned(),
            codec,
            content_length: None,
            expires_at: None,
            headers: Vec::new(),
            bitrate_kbps: None,
        }
    }

    /// The local path, when this source is not an HTTP URL.
    pub fn local_path(&self) -> Option<&Path> {
        if self.url.starts_with("http://") || self.url.starts_with("https://") {
            None
        } else {
            Some(Path::new(
                self.url.strip_prefix("file://").unwrap_or(&self.url),
            ))
        }
    }

    /// True when the URL expires within `margin`, so resolving it again now
    /// beats a 403 halfway through the track.
    pub fn expires_within(&self, margin: Duration) -> bool {
        self.expires_at
            .is_some_and(|at| at <= SystemTime::now() + margin)
    }

    /// Codec and bitrate for display, such as "opus 160 kbps".
    pub fn label(&self) -> String {
        let codec = match self.codec {
            Codec::Opus => "opus",
            Codec::Aac => "aac",
            Codec::Other => "audio",
        };
        match self.bitrate_kbps {
            Some(kbps) => format!("{codec} {kbps} kbps"),
            None => codec.to_owned(),
        }
    }

    /// Builds a source from one entry of yt-dlp's `formats` array. Returns
    /// `None` for video, DRM, HLS and storyboard formats.
    pub fn from_ytdlp_format(format: &Value) -> Option<Self> {
        let str_field = |key: &str| format.get(key).and_then(Value::as_str);
        if str_field("vcodec").is_some_and(|v| v != "none")
            || str_field("protocol") != Some("https")
            || format.get("has_drm").and_then(Value::as_bool) == Some(true)
        {
            return None;
        }
        let acodec = str_field("acodec").filter(|a| *a != "none")?;
        let url = str_field("url")?.to_owned();
        let mime = match str_field("ext") {
            Some("webm") => "audio/webm",
            Some("m4a" | "mp4") => "audio/mp4",
            _ => "application/octet-stream",
        };
        let headers = format
            .get("http_headers")
            .and_then(Value::as_object)
            .map(|h| {
                h.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            expires_at: url_expiry(&url),
            url,
            mime: mime.to_owned(),
            codec: Codec::from_acodec(acodec),
            content_length: format.get("filesize").and_then(Value::as_u64),
            headers,
            bitrate_kbps: format
                .get("abr")
                .and_then(Value::as_f64)
                .map(|abr| abr.round() as u32),
        })
    }

    /// Picks the best audio format from a `yt-dlp -J` document: Premium opus
    /// (774), opus (251), Premium AAC (141), AAC (140), then whatever has the
    /// highest bitrate.
    pub fn best_from_ytdlp(info: &Value) -> Option<Self> {
        const PREFERRED: [&str; 4] = ["774", "251", "141", "140"];
        let formats = info.get("formats")?.as_array()?;
        let candidates = formats.iter().filter_map(|f| {
            let source = Self::from_ytdlp_format(f)?;
            let id = f
                .get("format_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // Exact match, so a dynamic-range-compressed "251-drc" ranks
            // after the original.
            let rank = PREFERRED
                .iter()
                .position(|p| *p == id)
                .unwrap_or(PREFERRED.len());
            Some((
                rank,
                source.codec != Codec::Opus,
                std::cmp::Reverse(source.bitrate_kbps),
                source,
            ))
        });
        candidates
            .min_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)))
            .map(|c| c.3)
    }
}

fn url_expiry(url: &str) -> Option<SystemTime> {
    let query = url.split_once('?')?.1;
    let secs = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("expire="))?
        .parse()
        .ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn format(id: &str, ext: &str, acodec: &str, abr: f64) -> Value {
        json!({
            "format_id": id, "ext": ext, "acodec": acodec, "vcodec": "none", "abr": abr,
            "protocol": "https", "filesize": 1000,
            "url": format!("https://rr1.googlevideo.com/videoplayback?expire=1791283334&itag={id}"),
            "http_headers": { "User-Agent": "test" },
        })
    }

    #[test]
    fn prefers_opus_then_aac_then_bitrate() {
        let info = json!({ "formats": [
            format("140", "m4a", "mp4a.40.2", 129.5),
            format("251", "webm", "opus", 128.9),
            format("249", "webm", "opus", 46.2),
            { "format_id": "18", "vcodec": "avc1", "acodec": "mp4a.40.2", "protocol": "https", "url": "x" },
        ]});
        let best = StreamSource::best_from_ytdlp(&info).unwrap();
        assert_eq!(best.codec, Codec::Opus);
        assert_eq!(best.mime, "audio/webm");
        assert_eq!(best.bitrate_kbps, Some(129));
        assert_eq!(best.content_length, Some(1000));
        assert_eq!(best.headers, [("User-Agent".to_owned(), "test".to_owned())]);
        assert_eq!(
            best.expires_at,
            Some(UNIX_EPOCH + Duration::from_secs(1791283334))
        );

        let premium = json!({ "formats": [format("251", "webm", "opus", 128.9), format("774", "webm", "opus", 256.0)] });
        assert_eq!(
            StreamSource::best_from_ytdlp(&premium)
                .unwrap()
                .bitrate_kbps,
            Some(256)
        );

        let only_aac = json!({ "formats": [format("139", "m4a", "mp4a.40.5", 48.0), format("140", "m4a", "mp4a.40.2", 129.5)] });
        assert_eq!(
            StreamSource::best_from_ytdlp(&only_aac).unwrap().label(),
            "aac 130 kbps"
        );
    }

    #[test]
    fn local_paths() {
        let source = StreamSource::file("/music/a.m4a");
        assert_eq!(source.codec, Codec::Aac);
        assert_eq!(source.local_path(), Some(Path::new("/music/a.m4a")));
        let url = StreamSource {
            url: "file:///music/a.webm".into(),
            ..source
        };
        assert_eq!(url.local_path(), Some(Path::new("/music/a.webm")));
    }
}
