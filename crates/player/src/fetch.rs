//! HTTP range streaming into a sparse block cache.
//!
//! A fetcher thread per stream downloads ahead of the reader in block-aligned
//! range requests and parks once the read-ahead window is full. The reader
//! side is a blocking `Read + Seek` that symphonia demuxes from. Seeking into
//! a block that is already cached costs nothing; seeking past the download
//! makes the fetcher drop its request and open a new range at the reader.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use reqwest::header::{CONTENT_RANGE, HeaderMap, HeaderName, HeaderValue, RANGE};
use symphonia::core::io::MediaSource;

use crate::error::PlayerError;
use crate::source::StreamSource;

const BLOCK_SIZE: usize = 64 * 1024;
/// googlevideo throttles or refuses much larger single ranges.
pub(crate) const RANGE_SIZE: u64 = 10 * 1024 * 1024;
const READ_AHEAD: u64 = 32 * 1024 * 1024;
const MAX_CACHED: u64 = 64 * 1024 * 1024;
/// A miss up to this far past the block being downloaded waits for the
/// download instead of opening a new range, so short forward skips (fMP4
/// fragment headers during a seek) don't cost a round trip each.
const JUMP_TOLERANCE_BLOCKS: usize = 16;
const MAX_ATTEMPTS: u32 = 5;
const RETRY_BASE: Duration = Duration::from_millis(250);
/// Bytes past the read position that count as "ready": a few seconds of
/// audio at any YouTube bitrate.
const READY_BYTES: u64 = 64 * 1024;

pub(crate) fn http_client() -> Result<Client, PlayerError> {
    Client::builder()
        // Audio is already compressed, and a gzip body breaks byte ranges.
        .no_gzip()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| PlayerError::Network(e.to_string()))
}

struct Cache {
    blocks: Vec<Option<Box<[u8]>>>,
    cached: u64,
    read_pos: u64,
    error: Option<PlayerError>,
    closed: bool,
}

struct Shared {
    len: u64,
    cache: Mutex<Cache>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn block_count(&self) -> usize {
        self.len.div_ceil(BLOCK_SIZE as u64) as usize
    }

    fn block_len(&self, block: usize) -> usize {
        (self.len - block as u64 * BLOCK_SIZE as u64).min(BLOCK_SIZE as u64) as usize
    }
}

/// Read-side view of a stream's download, for buffering reports.
#[derive(Clone)]
pub(crate) struct StreamStatus(Arc<Shared>);

impl StreamStatus {
    pub fn len(&self) -> u64 {
        self.0.len
    }

    /// Byte offset up to which data is cached contiguously from the reader.
    pub fn buffered_until(&self) -> u64 {
        let cache = self.0.lock();
        let first = (cache.read_pos / BLOCK_SIZE as u64) as usize;
        match (first..cache.blocks.len()).find(|&b| cache.blocks[b].is_none()) {
            Some(missing) => missing as u64 * BLOCK_SIZE as u64,
            None => self.0.len,
        }
    }

    /// True when the next read will not wait on the network.
    pub fn is_ready(&self) -> bool {
        let read_pos = self.0.lock().read_pos;
        let until = self.buffered_until();
        until == self.0.len || until.saturating_sub(read_pos) >= READY_BYTES
    }

    pub fn error(&self) -> Option<PlayerError> {
        self.0.lock().error.clone()
    }
}

/// The reader symphonia pulls from. Dropping it stops the fetcher thread.
pub(crate) struct RangeReader {
    shared: Arc<Shared>,
    pos: u64,
}

impl RangeReader {
    pub fn open(
        client: &Client,
        source: &StreamSource,
    ) -> Result<(Self, StreamStatus), PlayerError> {
        Self::open_with(client, source, RANGE_SIZE)
    }

    pub(crate) fn open_with(
        client: &Client,
        source: &StreamSource,
        range_size: u64,
    ) -> Result<(Self, StreamStatus), PlayerError> {
        let mut headers = HeaderMap::new();
        for (name, value) in &source.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| PlayerError::Unsupported(e.to_string()))?;
            let value = HeaderValue::from_str(value)
                .map_err(|e| PlayerError::Unsupported(e.to_string()))?;
            headers.insert(name, value);
        }
        let fetcher = Fetcher {
            client: client.clone(),
            url: source.url.clone(),
            headers,
            range_blocks: (range_size / BLOCK_SIZE as u64).max(1) as usize,
            shared: None,
        };

        let first_end = source
            .content_length
            .map_or(range_size, |len| len.min(range_size));
        let response = fetcher.request_with_retries(0, first_end)?;
        let len = total_length(&response)
            .or(source.content_length)
            .ok_or_else(|| PlayerError::Unsupported("server reported no content length".into()))?;
        if len == 0 {
            return Err(PlayerError::Unsupported("empty stream".into()));
        }

        let shared = Arc::new(Shared {
            len,
            cache: Mutex::new(Cache {
                blocks: vec![None; len.div_ceil(BLOCK_SIZE as u64) as usize],
                cached: 0,
                read_pos: 0,
                error: None,
                closed: false,
            }),
            changed: Condvar::new(),
        });
        let first_end = first_end.min(len);
        let fetcher = Fetcher {
            shared: Some(shared.clone()),
            ..fetcher
        };
        thread::Builder::new()
            .name("formalmusic-fetch".into())
            .spawn(move || fetcher.run(response, first_end))
            .map_err(|e| PlayerError::Network(e.to_string()))?;

        Ok((
            Self {
                shared: shared.clone(),
                pos: 0,
            },
            StreamStatus(shared),
        ))
    }
}

impl Drop for RangeReader {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.changed.notify_all();
    }
}

impl Read for RangeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.pos >= self.shared.len {
            return Ok(0);
        }
        let block = (self.pos / BLOCK_SIZE as u64) as usize;
        let offset = (self.pos % BLOCK_SIZE as u64) as usize;
        let mut cache = self.shared.lock();
        if cache.read_pos / BLOCK_SIZE as u64 != block as u64 {
            self.shared.changed.notify_all();
        }
        cache.read_pos = self.pos;
        loop {
            if let Some(data) = &cache.blocks[block] {
                let n = out.len().min(data.len() - offset);
                out[..n].copy_from_slice(&data[offset..offset + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            if let Some(err) = &cache.error {
                return Err(io::Error::other(err.clone()));
            }
            self.shared.changed.notify_all();
            cache = self
                .shared
                .changed
                .wait(cache)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Seek for RangeReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.shared.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = target
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        Ok(self.pos)
    }
}

impl MediaSource for RangeReader {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.shared.len)
    }
}

/// A failed request, and whether trying again could help.
struct FetchError {
    error: PlayerError,
    transient: bool,
}

impl FetchError {
    fn transient(error: PlayerError) -> Self {
        Self {
            error,
            transient: true,
        }
    }

    fn fatal(error: PlayerError) -> Self {
        Self {
            error,
            transient: false,
        }
    }
}

enum Streamed {
    /// Reached the end of the range, or stopped because the reader moved.
    Done,
    Closed,
}

struct Fetcher {
    client: Client,
    url: String,
    headers: HeaderMap,
    range_blocks: usize,
    shared: Option<Arc<Shared>>,
}

impl Fetcher {
    fn shared(&self) -> &Shared {
        self.shared.as_deref().expect("fetcher runs after open")
    }

    fn request(&self, start: u64, end: u64) -> Result<Response, FetchError> {
        let response = self
            .client
            .get(&self.url)
            .headers(self.headers.clone())
            .header(RANGE, format!("bytes={start}-{}", end - 1))
            .send()
            .map_err(|e| FetchError::transient(PlayerError::Network(e.to_string())))?;
        match response.status() {
            StatusCode::PARTIAL_CONTENT => Ok(response),
            StatusCode::OK if start == 0 => Ok(response),
            StatusCode::OK => Err(FetchError::fatal(PlayerError::Unsupported(
                "server ignored the range header".into(),
            ))),
            StatusCode::FORBIDDEN | StatusCode::GONE => {
                Err(FetchError::fatal(PlayerError::Expired))
            }
            status if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS => {
                Err(FetchError::transient(PlayerError::Http(status.as_u16())))
            }
            status => Err(FetchError::fatal(PlayerError::Http(status.as_u16()))),
        }
    }

    fn request_with_retries(&self, start: u64, end: u64) -> Result<Response, PlayerError> {
        let mut attempt = 0;
        loop {
            match self.request(start, end) {
                Ok(response) => return Ok(response),
                Err(e) if e.transient && attempt + 1 < MAX_ATTEMPTS => {
                    tracing::debug!(error = %e.error, attempt, "range request failed, retrying");
                    thread::sleep(RETRY_BASE * 2u32.pow(attempt));
                    attempt += 1;
                }
                Err(e) => return Err(e.error),
            }
        }
    }

    fn run(self, first: Response, first_end: u64) {
        let mut attempt = 0;
        let mut next = Some((first, 0, first_end));
        loop {
            let (response, start_block, end) = match next.take() {
                Some(inflight) => inflight,
                None => {
                    let Some((block, end)) = self.next_range() else {
                        return;
                    };
                    match self.request(block as u64 * BLOCK_SIZE as u64, end) {
                        Ok(response) => (response, block, end),
                        Err(e) => {
                            if !self.retry(e, &mut attempt) {
                                return;
                            }
                            continue;
                        }
                    }
                }
            };
            match self.stream(response, start_block, end) {
                Ok(Streamed::Done) => attempt = 0,
                Ok(Streamed::Closed) => return,
                Err(e) => {
                    if !self.retry(e, &mut attempt) {
                        return;
                    }
                }
            }
        }
    }

    /// Sleeps before the next attempt, or records the error for the reader
    /// and returns false when giving up.
    fn retry(&self, err: FetchError, attempt: &mut u32) -> bool {
        if err.transient && *attempt + 1 < MAX_ATTEMPTS {
            tracing::debug!(error = %err.error, attempt = *attempt, "range request failed, retrying");
            thread::sleep(RETRY_BASE * 2u32.pow(*attempt));
            *attempt += 1;
            return true;
        }
        tracing::warn!(error = %err.error, "stream download failed");
        self.shared().lock().error = Some(err.error);
        self.shared().changed.notify_all();
        false
    }

    /// The next block-aligned range to download, waiting until the reader
    /// needs one. `None` once the reader is gone.
    fn next_range(&self) -> Option<(usize, u64)> {
        let shared = self.shared();
        let count = shared.block_count();
        let mut cache = shared.lock();
        loop {
            if cache.closed {
                return None;
            }
            let first = (cache.read_pos / BLOCK_SIZE as u64) as usize;
            let window = ((cache.read_pos + READ_AHEAD) / BLOCK_SIZE as u64) as usize + 1;
            if let Some(start) = (first..window.min(count)).find(|&b| cache.blocks[b].is_none()) {
                let cap = (start + self.range_blocks).min(count);
                let end_block = (start..cap)
                    .find(|&b| cache.blocks[b].is_some())
                    .unwrap_or(cap);
                let end = (end_block as u64 * BLOCK_SIZE as u64).min(shared.len);
                return Some((start, end));
            }
            cache = shared
                .changed
                .wait(cache)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    fn stream(
        &self,
        mut response: Response,
        start_block: usize,
        end: u64,
    ) -> Result<Streamed, FetchError> {
        let shared = self.shared();
        let mut block = start_block;
        let mut pending = Vec::with_capacity(BLOCK_SIZE);
        let mut pos = block as u64 * BLOCK_SIZE as u64;
        let mut chunk = [0u8; 16 * 1024];
        while pos < end {
            let want = chunk.len().min((end - pos) as usize);
            let n = response
                .read(&mut chunk[..want])
                .map_err(|e| FetchError::transient(PlayerError::Network(e.to_string())))?;
            if n == 0 {
                return Err(FetchError::transient(PlayerError::Network(
                    "connection closed mid-range".into(),
                )));
            }
            pos += n as u64;
            let mut data = &chunk[..n];
            while !data.is_empty() {
                let take = data.len().min(shared.block_len(block) - pending.len());
                pending.extend_from_slice(&data[..take]);
                data = &data[take..];
                if pending.len() == shared.block_len(block) {
                    let full = std::mem::replace(&mut pending, Vec::with_capacity(BLOCK_SIZE));
                    match self.publish(block, full.into_boxed_slice()) {
                        Publish::Continue => block += 1,
                        Publish::Redirect => return Ok(Streamed::Done),
                        Publish::Closed => return Ok(Streamed::Closed),
                    }
                }
            }
        }
        Ok(Streamed::Done)
    }

    fn publish(&self, block: usize, data: Box<[u8]>) -> Publish {
        let shared = self.shared();
        let mut cache = shared.lock();
        if cache.closed {
            return Publish::Closed;
        }
        if cache.blocks[block].is_none() {
            cache.cached += data.len() as u64;
            cache.blocks[block] = Some(data);
        }
        let read_block = (cache.read_pos / BLOCK_SIZE as u64) as usize;
        evict(&mut cache, read_block);
        shared.changed.notify_all();

        let next = block + 1;
        let reader_waits_elsewhere = cache.blocks.get(read_block).is_some_and(Option::is_none)
            && (read_block < next || read_block > next + JUMP_TOLERANCE_BLOCKS);
        let next_cached = cache.blocks.get(next).is_some_and(Option::is_some);
        let beyond_window = next as u64 * BLOCK_SIZE as u64 > cache.read_pos + READ_AHEAD;
        if reader_waits_elsewhere || next_cached || beyond_window {
            Publish::Redirect
        } else {
            Publish::Continue
        }
    }
}

enum Publish {
    Continue,
    /// Stop this range; the next one is chosen from the reader's position.
    Redirect,
    Closed,
}

/// Drops the blocks furthest behind the reader once the cache is over budget.
/// Tracks are a few MB, so this only matters for hour-long mixes.
fn evict(cache: &mut Cache, read_block: usize) {
    let mut block = 0;
    while cache.cached > MAX_CACHED && block < read_block {
        if let Some(data) = cache.blocks[block].take() {
            cache.cached -= data.len() as u64;
        }
        block += 1;
    }
}

/// The full length from `Content-Range: bytes a-b/total`, or the body length
/// of a plain 200.
fn total_length(response: &Response) -> Option<u64> {
    match response.headers().get(CONTENT_RANGE) {
        Some(range) => range.to_str().ok()?.rsplit_once('/')?.1.parse().ok(),
        None if response.status() == StatusCode::OK => response.content_length(),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A tiny HTTP/1.1 server with byte-range support that records every
    /// range it served.
    struct TestServer {
        url: String,
        ranges: Arc<Mutex<Vec<(u64, u64)>>>,
    }

    #[derive(Clone, Copy)]
    enum Behaviour {
        Normal,
        Forbidden,
        /// 503 for the first n requests.
        Unavailable(usize),
    }

    impl TestServer {
        fn start(body: Vec<u8>, behaviour: Behaviour) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/audio", listener.local_addr().unwrap());
            let ranges = Arc::new(Mutex::new(Vec::new()));
            let body = Arc::new(body);
            let failures = Arc::new(AtomicUsize::new(0));
            let served = ranges.clone();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (body, served, failures) = (body.clone(), served.clone(), failures.clone());
                    thread::spawn(move || serve(stream, &body, &served, &failures, behaviour));
                }
            });
            Self { url, ranges }
        }

        fn source(&self, content_length: Option<u64>) -> StreamSource {
            StreamSource {
                url: self.url.clone(),
                mime: "audio/webm".into(),
                codec: crate::Codec::Opus,
                content_length,
                expires_at: None,
                headers: vec![("User-Agent".into(), "formalmusic-test".into())],
                bitrate_kbps: None,
            }
        }
    }

    fn serve(
        stream: TcpStream,
        body: &[u8],
        served: &Mutex<Vec<(u64, u64)>>,
        failures: &AtomicUsize,
        behaviour: Behaviour,
    ) {
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        loop {
            let mut range = None;
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 {
                    return;
                }
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    let (a, b) = value.split_once('-').unwrap();
                    range = Some((a.parse::<u64>().unwrap(), b.parse::<u64>().unwrap()));
                }
            }
            let status = match behaviour {
                Behaviour::Forbidden => Some("403 Forbidden"),
                Behaviour::Unavailable(n) if failures.fetch_add(1, Ordering::SeqCst) < n => {
                    Some("503 Service Unavailable")
                }
                _ => None,
            };
            if let Some(status) = status {
                let _ = write!(writer, "HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
                continue;
            }
            let (start, last) = range.unwrap();
            let last = last.min(body.len() as u64 - 1);
            served.lock().unwrap().push((start, last + 1));
            let slice = &body[start as usize..=last as usize];
            let total = body.len();
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{last}/{total}\r\nContent-Length: {}\r\n\r\n",
                slice.len()
            );
            if writer
                .write_all(head.as_bytes())
                .and_then(|_| writer.write_all(slice))
                .is_err()
            {
                return;
            }
        }
    }

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[test]
    fn reads_everything_in_bounded_ranges() {
        let data = body(1_000_000);
        let server = TestServer::start(data.clone(), Behaviour::Normal);
        let client = http_client().unwrap();
        let range_size = 2 * BLOCK_SIZE as u64;
        let (mut reader, status) =
            RangeReader::open_with(&client, &server.source(None), range_size).unwrap();
        assert_eq!(status.len(), data.len() as u64);

        let mut read = Vec::new();
        reader.read_to_end(&mut read).unwrap();
        assert_eq!(read, data);
        let ranges = server.ranges.lock().unwrap().clone();
        assert!(ranges.len() >= data.len() / range_size as usize);
        assert!(
            ranges.iter().all(|(a, b)| b - a <= range_size),
            "{ranges:?}"
        );
        assert!(ranges.iter().all(|(a, _)| a % BLOCK_SIZE as u64 == 0));
    }

    #[test]
    fn seek_past_the_download_opens_a_new_range() {
        let data = body(3_000_000);
        let server = TestServer::start(data.clone(), Behaviour::Normal);
        let client = http_client().unwrap();
        let (mut reader, _) = RangeReader::open_with(
            &client,
            &server.source(Some(data.len() as u64)),
            BLOCK_SIZE as u64,
        )
        .unwrap();

        let mut head = [0u8; 100];
        reader.read_exact(&mut head).unwrap();
        assert_eq!(head, data[..100]);

        let target = 2_500_000;
        reader.seek(SeekFrom::Start(target)).unwrap();
        let mut tail = vec![0u8; 4096];
        reader.read_exact(&mut tail).unwrap();
        assert_eq!(tail, data[target as usize..target as usize + 4096]);

        let target_block = target / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
        let ranges = server.ranges.lock().unwrap().clone();
        assert!(ranges.iter().any(|(a, _)| *a == target_block), "{ranges:?}");

        // Seeking back into what was already fetched reads from the cache.
        reader.seek(SeekFrom::Start(10)).unwrap();
        let mut again = [0u8; 50];
        reader.read_exact(&mut again).unwrap();
        assert_eq!(again, data[10..60]);
    }

    #[test]
    fn forbidden_is_reported_as_expired() {
        let server = TestServer::start(body(1000), Behaviour::Forbidden);
        let client = http_client().unwrap();
        let err = RangeReader::open(&client, &server.source(None))
            .err()
            .unwrap();
        assert_eq!(err, PlayerError::Expired);
    }

    #[test]
    fn expiry_mid_stream_surfaces_through_io_errors() {
        let shared = Arc::new(Shared {
            len: 10,
            cache: Mutex::new(Cache {
                blocks: vec![None],
                cached: 0,
                read_pos: 0,
                error: Some(PlayerError::Expired),
                closed: false,
            }),
            changed: Condvar::new(),
        });
        let mut reader = RangeReader { shared, pos: 0 };
        let err = reader.read(&mut [0u8; 4]).unwrap_err();
        assert_eq!(PlayerError::from_io(&err), PlayerError::Expired);
    }

    #[test]
    fn transient_errors_are_retried() {
        let data = body(200_000);
        let server = TestServer::start(data.clone(), Behaviour::Unavailable(2));
        let client = http_client().unwrap();
        let (mut reader, _) = RangeReader::open(&client, &server.source(None)).unwrap();
        let mut read = Vec::new();
        reader.read_to_end(&mut read).unwrap();
        assert_eq!(read, data);
    }

    #[test]
    fn length_comes_from_content_range() {
        let data = body(150_000);
        let server = TestServer::start(data.clone(), Behaviour::Normal);
        let client = http_client().unwrap();
        let (reader, status) =
            RangeReader::open_with(&client, &server.source(None), 65_536).unwrap();
        assert_eq!(status.len(), 150_000);
        assert_eq!(reader.byte_len(), Some(150_000));
        let ranges = server.ranges.lock().unwrap().clone();
        assert_eq!(ranges[0], (0, 65_536));
    }

    #[test]
    fn buffering_reaches_the_end() {
        let data = body(300_000);
        let server = TestServer::start(data.clone(), Behaviour::Normal);
        let client = http_client().unwrap();
        let (_reader, status) = RangeReader::open(&client, &server.source(None)).unwrap();
        for _ in 0..200 {
            if status.buffered_until() == status.len() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(status.buffered_until(), 300_000);
        assert!(status.is_ready());
        assert!(status.error().is_none());
    }
}
