//! The `player` response, read only for playback reporting and loudness.
//! Stream URLs are left to yt-dlp.

use super::missing;
use crate::Result;
use formalmusic_api::ApiError;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackTracking {
    pub video_id: String,
    /// Pinged once when playback starts; this is what lands the track in History.
    pub playback_url: String,
    /// Pinged periodically with the watched ranges.
    pub watchtime_url: Option<String>,
    /// How much louder than the reference level the track is, in dB. The web
    /// app turns the volume down by this much when normalisation is on.
    pub loudness_db: Option<f64>,
}

pub fn parse_player(video_id: &str, json: &Value) -> Result<PlaybackTracking> {
    let status = &json["playabilityStatus"];
    match status["status"].as_str() {
        Some("OK") => {}
        Some("LOGIN_REQUIRED") => return Err(ApiError::SignedOut),
        Some(other) => {
            let reason = status["reason"].as_str().unwrap_or(other);
            return Err(ApiError::Playback(format!("{video_id}: {reason}")));
        }
        None => return Err(missing("playabilityStatus.status")),
    }
    let tracking = &json["playbackTracking"];
    let url = |key: &str| tracking[key]["baseUrl"].as_str().map(str::to_owned);
    Ok(PlaybackTracking {
        video_id: video_id.to_owned(),
        playback_url: url("videostatsPlaybackUrl")
            .ok_or_else(|| missing("playbackTracking.videostatsPlaybackUrl"))?,
        watchtime_url: url("videostatsWatchtimeUrl"),
        loudness_db: json["playerConfig"]["audioConfig"]["loudnessDb"].as_f64(),
    })
}
