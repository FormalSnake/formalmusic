//! Resolves a real track with yt-dlp and plays it into the null sink.
//! Needs network and `yt-dlp` on `PATH`:
//!
//! ```text
//! cargo test -p formalmusic-player --test live -- --ignored
//! ```

use std::process::Command;
use std::time::{Duration, Instant};

use formalmusic_player::{Codec, OutputKind, Player, PlayerEvent, Status, StreamSource};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;

const VIDEO_ID: &str = "dQw4w9WgXcQ";

fn ytdlp_info() -> serde_json::Value {
    let url = format!("https://music.youtube.com/watch?v={VIDEO_ID}");
    let output = Command::new("yt-dlp")
        .args(["-J", "--no-warnings", &url])
        .output()
        .expect("yt-dlp on PATH");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Collects events until `done` returns true, failing on any error event.
fn wait_for(
    events: &mut Receiver<PlayerEvent>,
    timeout: Duration,
    mut done: impl FnMut(&PlayerEvent) -> bool,
) -> PlayerEvent {
    let deadline = Instant::now() + timeout;
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for an event");
        match events.try_recv() {
            Ok(PlayerEvent::Error { error, .. }) => panic!("player error: {error}"),
            Ok(event) if done(&event) => return event,
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryRecvError::Closed) => panic!("player gone"),
        }
    }
}

fn position(event: &PlayerEvent) -> Option<u64> {
    match event {
        PlayerEvent::Position { position_ms, .. } => Some(*position_ms),
        _ => None,
    }
}

fn play_ten_seconds(source: StreamSource) {
    let player = Player::with_output(OutputKind::Null {
        sample_rate: 48_000,
        channels: 2,
    })
    .unwrap();
    let mut events = player.subscribe();
    let started = Instant::now();
    player.load(source, 0, Some(4.5));
    wait_for(&mut events, Duration::from_secs(20), |e| {
        *e == PlayerEvent::StateChanged(Status::Playing)
    });
    println!("audible after {:?}", started.elapsed());

    let event = wait_for(&mut events, Duration::from_secs(30), |e| {
        position(e).is_some_and(|p| p >= 10_000)
    });
    let elapsed = started.elapsed();
    println!("10 s decoded and played in {elapsed:?}: {event:?}");

    // Seek forward past the buffered region and check the clock lands there.
    player.seek(120_000);
    let landed = wait_for(&mut events, Duration::from_secs(10), |e| {
        position(e).is_some()
    });
    let landed = position(&landed).unwrap();
    assert!(
        (120_000..120_500).contains(&landed),
        "seek landed at {landed}"
    );
    let later = wait_for(&mut events, Duration::from_secs(10), |e| {
        position(e).is_some_and(|p| p >= 121_000)
    });
    println!("after seek: {later:?}");

    // Paused, the position holds still.
    player.pause();
    wait_for(&mut events, Duration::from_secs(5), |e| {
        *e == PlayerEvent::StateChanged(Status::Paused)
    });
    std::thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_ok() {}
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        matches!(events.try_recv(), Err(TryRecvError::Empty)),
        "no position events while paused"
    );
}

#[test]
#[ignore = "needs network and yt-dlp"]
fn live_opus() {
    let source = StreamSource::best_from_ytdlp(&ytdlp_info()).expect("an audio format");
    assert_eq!(source.codec, Codec::Opus);
    play_ten_seconds(source);
}

#[test]
#[ignore = "needs network and yt-dlp"]
fn live_aac_resampled() {
    let info = ytdlp_info();
    let format = info["formats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["format_id"] == "140")
        .expect("itag 140");
    let source = StreamSource::from_ytdlp_format(format).unwrap();
    assert_eq!(source.codec, Codec::Aac);
    play_ten_seconds(source);
}
