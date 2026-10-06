//! Lyrics and animated covers from `formalmusic-extras`, with YouTube Music's
//! own lyrics entered into the race, and warmed for the playing and next
//! track so the expanded player opens with both ready.

use formalmusic_api::{ApiError, Event, Lyrics, Track};
use formalmusic_extras::{AnimatedCovers, Lyricist, LyricsRequest};
use formalmusic_innertube::Client;
use std::sync::Arc;
use tokio::sync::broadcast;

pub struct Extras {
    lyricist: Lyricist,
    covers: AnimatedCovers,
}

impl Extras {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            lyricist: Lyricist::new()?,
            covers: AnimatedCovers::new()?,
        })
    }

    /// The best lyrics any provider has. `known` is the track when the queue
    /// already holds it; otherwise the `next` response describes it.
    pub async fn lyrics(
        &self,
        client: &Client,
        video_id: &str,
        known: Option<Track>,
    ) -> Result<Option<Lyrics>, ApiError> {
        let next = client.next(Some(video_id), None).await?;
        let track = known.or_else(|| next.tracks.iter().find(|t| t.video_id == video_id).cloned());
        let youtube_music = match &next.lyrics_browse_id {
            Some(browse_id) => client
                .lyrics_by_browse_id(browse_id)
                .await
                .unwrap_or_else(|e| {
                    tracing::debug!(video_id, "youtube music lyrics failed: {e}");
                    None
                }),
            None => None,
        };
        let Some(track) = track else {
            return Ok(youtube_music);
        };
        let request = LyricsRequest {
            title: track.title.clone(),
            artists: track.artists.iter().map(|a| a.text.clone()).collect(),
            album: track.album.as_ref().map(|a| a.text.clone()),
            duration_ms: track.duration_ms,
            youtube_music: youtube_music.clone(),
        };
        match self.lyricist.lyrics(&request).await {
            Ok(lyrics) => Ok(lyrics),
            Err(e) => {
                tracing::debug!(video_id, "lyrics providers failed: {e}");
                Ok(youtube_music)
            }
        }
    }

    pub async fn animated_cover(
        &self,
        artist: &str,
        album: &str,
    ) -> Result<Option<String>, ApiError> {
        self.covers
            .animated_cover(artist, album)
            .await
            .map(|path| path.map(|p| p.to_string_lossy().into_owned()))
            .map_err(|e| ApiError::Network(e.to_string()))
    }

    async fn warm(&self, client: &Client, track: Track) {
        if let (Some(artist), Some(album)) = (track.artists.first(), &track.album) {
            let _ = self.animated_cover(&artist.text, &album.text).await;
        }
        let video_id = track.video_id.clone();
        let _ = self.lyrics(client, &video_id, Some(track)).await;
    }
}

/// Warms lyrics and covers whenever the playing track changes, for it and
/// the entry after it.
pub async fn warm_on_track_change(
    extras: Arc<Extras>,
    session: Arc<crate::session::Session>,
    playback: Arc<crate::playback::Playback>,
    mut events: broadcast::Receiver<Event>,
) {
    let mut last: Option<String> = None;
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let Event::Player(state) = event else {
            continue;
        };
        let Some(track) = state.track else { continue };
        if last.as_deref() == Some(track.video_id.as_str()) {
            continue;
        }
        last = Some(track.video_id.clone());
        let tracks = [Some(track), playback.next_track()];
        for track in tracks.into_iter().flatten() {
            let (extras, client) = (extras.clone(), session.client());
            tokio::spawn(async move { extras.warm(&client, track).await });
        }
    }
}
