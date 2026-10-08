//! MPRIS on the session bus as `org.mpris.MediaPlayer2.formalmusic`, so media
//! keys, the desktop's media widget and `playerctl` work without the app.

use crate::playback::{Playback, large_art};
use formalmusic_api::{Event, PlayerState, Repeat, Status};
use mpris_server::zbus::{Result, fdo};
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Signal, Time, TrackId, Volume,
};
use std::sync::Arc;
use tokio::sync::broadcast;

pub struct Mpris {
    playback: Arc<Playback>,
}

/// Exports the interfaces and keeps their properties in step with playback.
pub async fn start(
    playback: Arc<Playback>,
    events: broadcast::Receiver<Event>,
) -> Result<Arc<Server<Mpris>>> {
    let seeks = playback.subscribe_seeks();
    let server = Arc::new(Server::new("formalmusic", Mpris { playback }).await?);
    tokio::spawn(follow(server.clone(), events, seeks));
    Ok(server)
}

async fn follow(
    server: Arc<Server<Mpris>>,
    mut events: broadcast::Receiver<Event>,
    mut seeks: broadcast::Receiver<u64>,
) {
    let mut last: Vec<Property> = Vec::new();
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(Event::Player(_) | Event::Queue(_)) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    let current = properties(&server.imp().playback);
                    let changed: Vec<Property> = current.iter().filter(|p| !last.contains(p)).cloned().collect();
                    if !changed.is_empty() && let Err(e) = server.properties_changed(changed).await {
                        tracing::debug!("mpris properties: {e}");
                    }
                    last = current;
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            seek = seeks.recv() => match seek {
                Ok(position_ms) => {
                    let position = Time::from_millis(position_ms as i64);
                    if let Err(e) = server.emit(Signal::Seeked { position }).await {
                        tracing::debug!("mpris seeked: {e}");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

fn properties(playback: &Playback) -> Vec<Property> {
    let state = playback.player_state();
    let has_track = state.track.is_some();
    vec![
        Property::PlaybackStatus(playback_status(state.status)),
        Property::LoopStatus(loop_status(state.repeat)),
        Property::Shuffle(state.shuffle),
        Property::Volume(state.volume as Volume),
        Property::CanGoNext(playback.can_go_next()),
        Property::CanGoPrevious(has_track),
        Property::CanPlay(has_track),
        Property::CanSeek(has_track),
        Property::Metadata(metadata(&state, playback.current_uid())),
    ]
}

fn playback_status(status: Status) -> PlaybackStatus {
    match status {
        Status::Playing | Status::Loading => PlaybackStatus::Playing,
        Status::Paused => PlaybackStatus::Paused,
        Status::Stopped => PlaybackStatus::Stopped,
    }
}

fn loop_status(repeat: Repeat) -> LoopStatus {
    match repeat {
        Repeat::Off => LoopStatus::None,
        Repeat::All => LoopStatus::Playlist,
        Repeat::One => LoopStatus::Track,
    }
}

fn track_id(uid: u64) -> TrackId {
    TrackId::try_from(format!("/es/canarycoders/formalmusic/track/{uid}"))
        .unwrap_or(TrackId::NO_TRACK)
}

fn metadata(state: &PlayerState, uid: Option<u64>) -> Metadata {
    let (Some(track), Some(uid)) = (&state.track, uid) else {
        return Metadata::builder().trackid(TrackId::NO_TRACK).build();
    };
    let mut m = Metadata::new();
    m.set_trackid(Some(track_id(uid)));
    m.set_title(Some(track.title.clone()));
    m.set_artist(Some(track.artists.iter().map(|a| a.text.clone())));
    m.set_album(track.album.as_ref().map(|a| a.text.clone()));
    m.set_length(state.duration_ms.map(|ms| Time::from_millis(ms as i64)));
    m.set_art_url(track.thumbnails.last().map(|t| large_art(&t.url)));
    m.set_url(Some(format!(
        "https://music.youtube.com/watch?v={}",
        track.video_id
    )));
    m
}

impl RootInterface for Mpris {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "the daemon runs as a service".into(),
        ))
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> Result<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("FormalMusic".into())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("es.canarycoders.formalmusic".into())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl PlayerInterface for Mpris {
    async fn next(&self) -> fdo::Result<()> {
        self.playback.next();
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.playback.previous();
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        self.playback.pause("mpris");
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.playback.toggle("mpris");
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.playback.pause("mpris stop");
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        self.playback.resume();
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let target = self.playback.live_position() as i64 + offset.as_millis();
        let duration = self.playback.player_state().duration_ms;
        // The spec: seeking past the end acts like Next.
        if duration.is_some_and(|d| target > d as i64) {
            self.playback.next();
        } else {
            self.playback.seek(target.max(0) as u64);
        }
        Ok(())
    }

    async fn set_position(&self, track_id_: TrackId, position: Time) -> fdo::Result<()> {
        let current = self.playback.current_uid().map(track_id);
        let duration = self.playback.player_state().duration_ms;
        let in_range =
            position.as_millis() >= 0 && duration.is_none_or(|d| position.as_millis() <= d as i64);
        if current.as_ref() == Some(&track_id_) && in_range {
            self.playback.seek(position.as_millis() as u64);
        }
        Ok(())
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("open a track from the app".into()))
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(playback_status(self.playback.player_state().status))
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(loop_status(self.playback.player_state().repeat))
    }

    async fn set_loop_status(&self, loop_status: LoopStatus) -> Result<()> {
        self.playback.set_repeat(match loop_status {
            LoopStatus::None => Repeat::Off,
            LoopStatus::Playlist => Repeat::All,
            LoopStatus::Track => Repeat::One,
        });
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> Result<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.playback.player_state().shuffle)
    }

    async fn set_shuffle(&self, shuffle: bool) -> Result<()> {
        self.playback.set_shuffle(shuffle);
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(metadata(
            &self.playback.player_state(),
            self.playback.current_uid(),
        ))
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.playback.player_state().volume as Volume)
    }

    async fn set_volume(&self, volume: Volume) -> Result<()> {
        self.playback.set_volume(volume as f32);
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(Time::from_millis(self.playback.live_position() as i64))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(self.playback.can_go_next())
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(self.playback.player_state().track.is_some())
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(self.playback.player_state().track.is_some())
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.playback.player_state().track.is_some())
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_is_asked_large() {
        assert_eq!(
            large_art("https://lh3.googleusercontent.com/abc=w120-h120-l90-rj"),
            "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj"
        );
        let ytimg = "https://i.ytimg.com/vi/x/sddefault.jpg";
        assert_eq!(large_art(ytimg), ytimg);
    }
}
