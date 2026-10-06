//! Apple Music's looping album videos.
//!
//! The chain, all undocumented: an iTunes artist search and that artist's
//! album list resolve the name pair to an Apple Music album id; the anonymous
//! web-player token scraped from music.apple.com unlocks amp-api's
//! `editorialVideo`; its HLS master names a rendition whose `#EXT-X-MAP` is a
//! plain mp4, which is downloaded into the cache.
//!
//! Only definitive answers are remembered. A track with no animated cover is
//! a miss held in memory for the life of the value; a network failure is an
//! error and is retried on the next call.

mod hls;
mod itunes;
mod web;

use crate::{Error, Result, cache, http};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoints {
    pub itunes: String,
    pub web_player: String,
    pub amp_api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            itunes: "https://itunes.apple.com".into(),
            web_player: "https://music.apple.com".into(),
            amp_api: "https://amp-api.music.apple.com".into(),
        }
    }
}

pub struct AnimatedCovers {
    http: reqwest::Client,
    dir: PathBuf,
    endpoints: Endpoints,
    /// The web-player JWT. Memory only, rescraped when amp-api refuses it.
    token: AsyncMutex<Option<String>>,
    misses: Mutex<HashSet<String>>,
    pruned: OnceCell<()>,
}

impl AnimatedCovers {
    /// Caches under `$XDG_CACHE_HOME/formalmusic/animated-covers`.
    pub fn new() -> Result<Self> {
        Self::with_cache_dir(cache::dir("animated-covers"))
    }

    pub fn with_cache_dir(dir: PathBuf) -> Result<Self> {
        Self::build(dir, Endpoints::default())
    }

    fn build(dir: PathBuf, endpoints: Endpoints) -> Result<Self> {
        Ok(Self {
            http: http::client()?,
            dir,
            endpoints,
            token: AsyncMutex::new(None),
            misses: Mutex::new(HashSet::new()),
            pruned: OnceCell::new(),
        })
    }

    /// The local mp4 for the album, downloading it on first use. `Ok(None)`
    /// when Apple Music has no animated cover for it.
    pub async fn animated_cover(&self, artist: &str, album: &str) -> Result<Option<PathBuf>> {
        let key = cache::slug(&[artist, album]);
        if key.is_empty() || self.misses.lock().unwrap().contains(&key) {
            return Ok(None);
        }
        self.pruned.get_or_init(|| prune(&self.dir)).await;

        let path = self.dir.join(format!("{key}.mp4"));
        if tokio::fs::metadata(&path).await.is_ok_and(|m| m.len() > 0) {
            return Ok(Some(path));
        }
        if self.fetch(artist, album, &path).await? {
            return Ok(Some(path));
        }
        self.misses.lock().unwrap().insert(key);
        Ok(None)
    }

    /// Downloads the cover to `path`; `false` when there is none to download.
    async fn fetch(&self, artist: &str, album: &str, path: &Path) -> Result<bool> {
        let e = &self.endpoints;
        let search = self.http.get(format!("{}/search", e.itunes)).query(&[
            ("media", "music"),
            ("entity", "musicArtist"),
            ("limit", "5"),
            ("term", artist),
        ]);
        let Some(artist_id) =
            itunes::artist_id(&http::text(search, LOOKUP_TIMEOUT).await?, artist)?
        else {
            return Ok(false);
        };

        let albums = self.http.get(format!("{}/lookup", e.itunes)).query(&[
            ("id", artist_id.to_string().as_str()),
            ("entity", "album"),
            ("limit", "200"),
        ]);
        let Some(collection_id) =
            itunes::collection_id(&http::text(albums, LOOKUP_TIMEOUT).await?, album)?
        else {
            return Ok(false);
        };

        let Some(master_url) = self.editorial_video(collection_id).await? else {
            return Ok(false);
        };
        let master = http::text(self.http.get(&master_url), LOOKUP_TIMEOUT).await?;
        let Some(variant) = hls::pick_variant(&master).and_then(|v| hls::resolve(&master_url, v))
        else {
            return Ok(false);
        };
        let rendition = http::text(self.http.get(variant.clone()), LOOKUP_TIMEOUT).await?;
        let Some(mp4) =
            hls::map_uri(&rendition).and_then(|uri| hls::resolve(variant.as_str(), uri))
        else {
            return Ok(false);
        };

        self.download(mp4, path).await?;
        Ok(true)
    }

    /// Asks amp-api, scraping a token first if there is none. A 401 or 403
    /// means the token expired: it is dropped and the request retried once
    /// with a fresh one.
    async fn editorial_video(&self, collection_id: u64) -> Result<Option<String>> {
        let url = format!(
            "{}/v1/catalog/us/albums/{collection_id}?extend=editorialVideo",
            self.endpoints.amp_api
        );
        for attempt in 0..2 {
            let token = self.token(collection_id).await?;
            let request = self
                .http
                .get(&url)
                .bearer_auth(token)
                .header("Origin", &self.endpoints.web_player);
            let (status, body) = http::fetch(request, LOOKUP_TIMEOUT).await?;
            match status.as_u16() {
                200..=299 => return web::editorial_video(&body),
                401 | 403 if attempt == 0 => *self.token.lock().await = None,
                // Not in the US catalog.
                404 => return Ok(None),
                code => return Err(Error::Status { url, status: code }),
            }
        }
        unreachable!("the second attempt always returns")
    }

    async fn token(&self, collection_id: u64) -> Result<String> {
        let mut held = self.token.lock().await;
        if let Some(token) = held.as_ref() {
            return Ok(token.clone());
        }
        let base = &self.endpoints.web_player;
        let page = http::text(
            self.http.get(format!("{base}/us/album/{collection_id}")),
            LOOKUP_TIMEOUT,
        )
        .await?;
        let asset = web::asset_path(&page)
            .ok_or(Error::WebPlayer("album page references no script bundle"))?;
        let bundle = http::text(self.http.get(format!("{base}{asset}")), LOOKUP_TIMEOUT).await?;
        let token = web::token(&bundle).ok_or(Error::WebPlayer("no token in the script bundle"))?;
        *held = Some(token.to_owned());
        Ok(token.to_owned())
    }

    /// Writes to a `.part` file and renames it into place, so a reader never
    /// sees half a video and two downloads of one album never share a file.
    async fn download(&self, url: reqwest::Url, path: &Path) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let part = cache::temp_sibling(path, "part");
        let result = self.download_to(url, &part).await;
        match result {
            Ok(()) => tokio::fs::rename(&part, path).await.inspect_err(|_| {
                let _ = std::fs::remove_file(&part);
            })?,
            Err(_) => {
                let _ = tokio::fs::remove_file(&part).await;
            }
        }
        result
    }

    async fn download_to(&self, url: reqwest::Url, part: &Path) -> Result<()> {
        let mut res = self.http.get(url).timeout(DOWNLOAD_TIMEOUT).send().await?;
        if !res.status().is_success() {
            return Err(Error::Status {
                url: res.url().to_string(),
                status: res.status().as_u16(),
            });
        }
        let mut file = tokio::fs::File::create(part).await?;
        let mut len = 0;
        while let Some(chunk) = res.chunk().await? {
            len += chunk.len();
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        if len == 0 {
            return Err(Error::Malformed("empty video download"));
        }
        Ok(())
    }
}

/// Removes files untouched for more than 30 days. Exactly 30 days is not yet
/// stale. A missing directory has nothing to prune.
async fn prune(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    let now = SystemTime::now();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if meta.is_file() && meta.modified().is_ok_and(|m| is_stale(m, now)) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

fn is_stale(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified).is_ok_and(|age| age > MAX_AGE)
}

#[cfg(test)]
mod tests;
