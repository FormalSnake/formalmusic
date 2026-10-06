//! Switching the playing track to another recording of it, the way the Song
//! and Video switch does, into the null sink. Skips when ffmpeg is missing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use formalmusic_player::{OutputKind, Player, PlayerEvent, Status, StreamSource, TrackId};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;

/// A 20 s tone, and the same tone behind `lead` seconds of silence, standing
/// in for an album track and its music video with an intro.
fn fixture(name: &str, lead: f64) -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-audio");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    if !path.exists() {
        let filter = format!("adelay={}:all=1", (lead * 1000.) as u64);
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi"])
            .args(["-i", "sine=frequency=440:sample_rate=48000:duration=20"])
            .args(["-af", &filter, "-ac", "2", "-c:a", "aac", "-f", "mp4"])
            .arg(&path)
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
    }
    Some(path)
}

fn next_event(events: &mut Receiver<PlayerEvent>, deadline: Instant) -> PlayerEvent {
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for an event");
        match events.try_recv() {
            Ok(PlayerEvent::Error { error, .. }) => panic!("player error: {error}"),
            Ok(event) => return event,
            Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(10)),
            Err(TryRecvError::Closed) => panic!("player gone"),
        }
    }
}

fn position_of(event: &PlayerEvent, id: TrackId) -> Option<u64> {
    match event {
        PlayerEvent::Position {
            track, position_ms, ..
        } if *track == id => Some(*position_ms),
        _ => None,
    }
}

#[test]
fn switch_hands_over_at_the_same_moment_without_stopping() {
    let (Some(song), Some(video)) = (fixture("song.m4a", 0.), fixture("video.m4a", 3.)) else {
        return;
    };
    let player = Player::with_output(OutputKind::Null {
        sample_rate: 48_000,
        channels: 2,
    })
    .unwrap();
    let mut events = player.subscribe();
    let first = player.load(StreamSource::file(&song), 0, None);
    let deadline = Instant::now() + Duration::from_secs(20);
    let at = loop {
        if let Some(position) = position_of(&next_event(&mut events, deadline), first)
            && position >= 1_000
        {
            break position;
        }
    };

    let second = player.switch(StreamSource::file(&video), at + 3_000, 3_000, None);
    let mut started = false;
    loop {
        match next_event(&mut events, deadline) {
            PlayerEvent::TrackStarted { track, .. } if track == second => started = true,
            PlayerEvent::StateChanged(status) => {
                assert_eq!(status, Status::Playing, "playback never stops for a switch")
            }
            PlayerEvent::TrackEnded { track } => panic!("{track:?} ended"),
            event => {
                if let Some(position) = position_of(&event, second) {
                    assert!(started);
                    // The intro is skipped: the new version picks up three
                    // seconds further in, at about where the old one was.
                    assert!(
                        (at + 3_000..at + 5_000).contains(&position),
                        "{position} after leaving at {at}"
                    );
                    return;
                }
            }
        }
    }
}
