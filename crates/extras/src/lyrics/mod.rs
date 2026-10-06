//! Synced lyrics from several providers raced against each other.
//!
//! Apple Music (through paxsenix, matched via an iTunes search) gives word or
//! syllable timing. lrclib gives line timing. The caller can pass YouTube
//! Music's own lyrics, already fetched through innertube, as a third
//! candidate; paxsenix's own YouTube endpoints answer 403 now, so that is the
//! only YouTube source.
//!
//! The first answer with word timing wins outright. Otherwise the best of
//! what came back wins, ties going to the earlier provider (Apple Music,
//! lrclib, YouTube Music). A winner from the network providers is written to
//! disk only when none of them failed, so a track that got lrclib's line
//! timing during an Apple outage asks again next time instead of freezing on
//! the weaker answer.

mod apple;
mod disk;
mod lrc;
mod lrclib;
mod score;

use crate::{Error, Result, cache, http};
use formalmusic_api::{LyricLine, Lyrics};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinSet;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);
const APPLE_LYRICS_TIMEOUT: Duration = Duration::from_secs(10);
const LRCLIB_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default)]
pub struct LyricsRequest {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// What innertube returned for the track, if anything. Line-synced or
    /// plain; plain is only used when nothing else was found.
    pub youtube_music: Option<Lyrics>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoints {
    pub itunes: String,
    pub paxsenix: String,
    pub lrclib: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            itunes: "https://itunes.apple.com".into(),
            paxsenix: "https://lyrics.paxsenix.org".into(),
            lrclib: "https://lrclib.net".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Provider {
    Apple,
    Lrclib,
    YoutubeMusic,
}

type Outcome = Result<Option<Lyrics>>;

#[derive(Clone)]
pub struct Lyricist {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    dir: PathBuf,
    endpoints: Endpoints,
}

impl Lyricist {
    /// Caches under `$XDG_CACHE_HOME/formalmusic/lyrics`.
    pub fn new() -> Result<Self> {
        Self::with_cache_dir(cache::dir("lyrics"))
    }

    pub fn with_cache_dir(dir: PathBuf) -> Result<Self> {
        Self::build(dir, Endpoints::default())
    }

    fn build(dir: PathBuf, endpoints: Endpoints) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Inner {
                http: http::client()?,
                dir,
                endpoints,
            }),
        })
    }

    /// `Ok(None)` when no provider has the track. An error means no provider
    /// had it and at least one could not be reached, so nothing was cached and
    /// asking again later is worthwhile.
    pub async fn lyrics(&self, req: &LyricsRequest) -> Result<Option<Lyrics>> {
        let inner = &self.inner;
        let artist = req.artists.join(" ");
        let key = disk::key(
            &artist,
            &req.title,
            req.album.as_deref().unwrap_or_default(),
            req.duration_ms,
        );
        match disk::read(&inner.dir, &key).await {
            disk::Cached::Hit(lyrics) => return Ok(Some(lyrics)),
            disk::Cached::Miss => return Ok(usable(req.youtube_music.clone())),
            disk::Cached::Absent => {}
        }

        // The race goes on after the first word-timed answer to learn whether
        // anything failed, so it runs detached from the caller's wait.
        let (tx, rx) = oneshot::channel();
        tokio::spawn(Arc::clone(inner).race(req.clone(), key, tx));
        rx.await.map_err(|_| Error::Cancelled)?
    }
}

impl Inner {
    async fn race(self: Arc<Self>, req: LyricsRequest, key: String, tx: oneshot::Sender<Outcome>) {
        let mut set = JoinSet::new();
        let (me, r) = (Arc::clone(&self), req.clone());
        set.spawn(async move { (Provider::Apple, me.apple(&r).await) });
        let (me, r) = (Arc::clone(&self), req.clone());
        set.spawn(async move { (Provider::Lrclib, me.lrclib(&r).await) });

        let mut tx = Some(tx);
        let mut decided: Option<(Provider, Lyrics)> = None;
        let mut candidates: Vec<(Provider, Lyrics)> = Vec::new();
        let mut errors: Vec<Error> = Vec::new();
        while let Some(joined) = set.join_next().await {
            let (provider, outcome) = match joined {
                Ok(done) => done,
                Err(_) => {
                    errors.push(Error::Cancelled);
                    continue;
                }
            };
            match outcome {
                Ok(Some(lyrics)) => {
                    if decided.is_none() && quality(&lyrics.lines) == 2 {
                        decided = Some((provider, lyrics.clone()));
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(Ok(Some(lyrics.clone())));
                        }
                    }
                    candidates.push((provider, lyrics));
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!("lyrics provider {provider:?} failed: {e}");
                    errors.push(e);
                }
            }
        }

        candidates.sort_by_key(|(provider, _)| *provider);
        if let Some(ytm) = req
            .youtube_music
            .as_ref()
            .filter(|l| l.synced && !l.lines.is_empty())
        {
            candidates.push((Provider::YoutubeMusic, ytm.clone()));
        }
        let winner = decided.or_else(|| pick_best(candidates));

        if errors.is_empty() {
            let written = match &winner {
                Some((Provider::YoutubeMusic, _)) | None => disk::write_miss(&self.dir, &key).await,
                Some((_, lyrics)) => disk::write_hit(&self.dir, &key, lyrics).await,
            };
            if let Err(e) = written {
                tracing::warn!("could not write lyrics cache for {key}: {e}");
            }
        }

        let result = match winner {
            Some((_, lyrics)) => Ok(Some(lyrics)),
            None => match usable(req.youtube_music) {
                Some(plain) => Ok(Some(plain)),
                None if errors.is_empty() => Ok(None),
                None => Err(errors.swap_remove(0)),
            },
        };
        if let Some(tx) = tx.take() {
            let _ = tx.send(result);
        }
    }

    async fn apple(&self, req: &LyricsRequest) -> Outcome {
        let e = &self.endpoints;
        let query = format!("{} {}", req.title, req.artists.join(" "))
            .trim()
            .to_owned();
        let search = self.http.get(format!("{}/search", e.itunes)).query(&[
            ("term", query.as_str()),
            ("entity", "song"),
            ("limit", "8"),
            ("country", "US"),
        ]);
        let songs = http::fetch_ok(search, SEARCH_TIMEOUT)
            .await?
            .map(|b| apple::parse_search(&b));
        let songs = songs.unwrap_or_default();
        let Some(track_id) =
            apple::best_song(&songs, &query, req.duration_ms).and_then(|s| s.track_id)
        else {
            return Ok(None);
        };

        let lyrics = self
            .http
            .get(format!("{}/apple-music/lyrics", e.paxsenix))
            .query(&[("id", track_id.to_string())]);
        let Some(body) = http::fetch_ok(lyrics, APPLE_LYRICS_TIMEOUT).await? else {
            return Ok(None);
        };
        Ok(assemble("Apple Music", apple::parse_lyrics(&body)))
    }

    async fn lrclib(&self, req: &LyricsRequest) -> Outcome {
        let base = &self.endpoints.lrclib;
        let artist = req.artists.first().cloned().unwrap_or_default();

        let mut get = vec![
            ("track_name", req.title.clone()),
            ("artist_name", artist.clone()),
        ];
        if let Some(album) = req.album.as_ref().filter(|a| !a.is_empty()) {
            get.push(("album_name", album.clone()));
        }
        if let Some(ms) = req.duration_ms.filter(|ms| *ms > 0) {
            get.push(("duration", ((ms as f64) / 1000.0).round().to_string()));
        }
        let request = self.http.get(format!("{base}/api/get")).query(&get);
        if let Some(body) = http::fetch_ok(request, LRCLIB_TIMEOUT).await?
            && let Some(found) = assemble("LRCLIB", lrclib::pick_synced(&body))
        {
            return Ok(Some(found));
        }

        let search = [
            ("track_name", req.title.as_str()),
            ("artist_name", artist.as_str()),
        ];
        let request = self.http.get(format!("{base}/api/search")).query(&search);
        let Some(body) = http::fetch_ok(request, LRCLIB_TIMEOUT).await? else {
            return Ok(None);
        };
        Ok(assemble("LRCLIB", lrclib::pick_synced(&body)))
    }
}

fn assemble(source: &str, lines: Vec<LyricLine>) -> Option<Lyrics> {
    if lines.is_empty() {
        return None;
    }
    Some(Lyrics {
        source: Some(source.to_owned()),
        word_synced: quality(&lines) == 2,
        lines,
        synced: true,
    })
}

/// 2 when any line has more than one timed word, 1 for line timing alone, 0
/// for nothing.
pub(crate) fn quality(lines: &[LyricLine]) -> u8 {
    if lines.is_empty() {
        0
    } else if lines.iter().any(|l| l.words.len() > 1) {
        2
    } else {
        1
    }
}

/// Highest quality wins and the earlier candidate takes a tie. Quality 0 never
/// wins.
fn pick_best(candidates: Vec<(Provider, Lyrics)>) -> Option<(Provider, Lyrics)> {
    let mut best: Option<(u8, (Provider, Lyrics))> = None;
    for candidate in candidates {
        let q = quality(&candidate.1.lines);
        if q > 0 && best.as_ref().is_none_or(|(top, _)| q > *top) {
            best = Some((q, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

fn usable(lyrics: Option<Lyrics>) -> Option<Lyrics> {
    lyrics.filter(|l| !l.lines.is_empty())
}

#[cfg(test)]
mod tests;
