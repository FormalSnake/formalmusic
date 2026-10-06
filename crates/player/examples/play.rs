//! Manual playback check.
//!
//! ```text
//! cargo run -p formalmusic-player --example play -- <url | path | --yt VIDEO_ID> [--null] [--start MS]
//!     [--loudness DB] [--next <path | --yt VIDEO_ID>] [--crossfade MS]
//! ```
//!
//! `--yt` runs `yt-dlp -J` and picks the best audio format. `--next` preloads
//! a second track when the first asks for it. `--null` plays
//! into a timer instead of a sound card. While playing, type `p` to toggle
//! pause, `s SECONDS` to seek, `v 0.5` for volume, `q` to quit.

use std::io::BufRead;
use std::process::Command;

use anyhow::{Context, bail};
use formalmusic_player::{OutputKind, Player, PlayerEvent, Status, StreamSource};

fn resolve(video_id: &str) -> anyhow::Result<StreamSource> {
    let url = format!("https://music.youtube.com/watch?v={video_id}");
    let output = Command::new("yt-dlp")
        .args(["-J", "--no-warnings", &url])
        .output()
        .context("running yt-dlp")?;
    if !output.status.success() {
        bail!("yt-dlp failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let info: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    StreamSource::best_from_ytdlp(&info).context("no audio format")
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let mut args = std::env::args().skip(1);
    let (mut source, mut null, mut start_ms, mut loudness) = (None, false, 0, None);
    let (mut next, mut crossfade, mut target_next) = (None, 0, false);
    while let Some(arg) = args.next() {
        let slot = if std::mem::take(&mut target_next) {
            &mut next
        } else {
            &mut source
        };
        match arg.as_str() {
            "--next" => target_next = true,
            "--crossfade" => crossfade = args.next().context("--crossfade needs ms")?.parse()?,
            "--yt" => *slot = Some(resolve(&args.next().context("--yt needs a video id")?)?),
            "--null" => null = true,
            "--start" => start_ms = args.next().context("--start needs ms")?.parse()?,
            "--loudness" => loudness = Some(args.next().context("--loudness needs dB")?.parse()?),
            url if url.starts_with("http") => {
                *slot = Some(StreamSource {
                    url: url.into(),
                    ..StreamSource::file("stream.webm")
                })
            }
            path => *slot = Some(StreamSource::file(path)),
        }
    }
    let source = source.context(
        "usage: play <url | path | --yt VIDEO_ID> [--null] [--start MS] [--loudness DB]",
    )?;
    println!("playing {} ({})", source.label(), source.mime);

    let output = if null {
        OutputKind::Null {
            sample_rate: 48_000,
            channels: 2,
        }
    } else {
        OutputKind::Default
    };
    let player = Player::with_output(output)?;
    let mut events = player.subscribe();
    player.set_crossfade(crossfade);
    player.load(source, start_ms, loudness);

    let preloader = player.clone();
    std::thread::spawn(move || {
        while let Ok(event) = events.blocking_recv() {
            println!("{event:?}");
            if matches!(event, PlayerEvent::NeedsNext { .. })
                && let Some(next) = next.take()
            {
                preloader.preload_next(next, None);
            }
            if matches!(event, PlayerEvent::StateChanged(Status::Stopped)) {
                std::process::exit(0);
            }
        }
    });

    let mut paused = false;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("p"), _) => {
                paused = !paused;
                if paused {
                    player.pause()
                } else {
                    player.play()
                }
            }
            (Some("s"), Some(secs)) => player.seek((secs.parse::<f64>()? * 1000.0) as u64),
            (Some("v"), Some(volume)) => player.set_volume(volume.parse()?),
            (Some("q"), _) => break,
            _ => println!("p | s SECONDS | v VOLUME | q"),
        }
    }
    Ok(())
}
