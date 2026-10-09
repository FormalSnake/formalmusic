//! A loopback HTTP server that hands ffmpeg a queued music video's picture,
//! which kopuzd serves only as byte ranges over its own socket. ffmpeg opens
//! the URL and seeks it with `Range` like any file on the web; each request
//! is answered from kopuzd chunk by chunk until ffmpeg has had enough.
//!
//! It listens on 127.0.0.1 only, under a path no other process can guess,
//! and stops when the [`Relay`] is dropped.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::backend::Backend;

/// Bytes asked of kopuzd per call, which it caps at 2 MiB anyway.
const CHUNK: u64 = 1024 * 1024;
/// A request head bigger than this is not ffmpeg's.
const MAX_HEAD: usize = 16 * 1024;

pub struct Relay {
    pub url: String,
    task: JoinHandle<()>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Relay {
    /// Serves the picture of the queued track `key`.
    pub async fn start(backend: Arc<dyn Backend>, key: String) -> std::io::Result<Relay> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let secret = format!("{:016x}", rand_u64());
        let url = format!("http://127.0.0.1:{port}/{secret}");
        let path = format!("/{secret}");
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (backend, key, path) = (backend.clone(), key.clone(), path.clone());
                tokio::spawn(async move {
                    if let Err(error) = serve(stream, &*backend, &key, &path).await {
                        tracing::debug!("video relay: {error}");
                    }
                });
            }
        });
        Ok(Relay { url, task })
    }
}

/// Not for secrecy against this user, only so another local user cannot
/// stumble on the port and read the stream.
fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    hasher.finish()
}

/// What a request asks for: the path, and where its range starts and ends
/// (inclusive), if it has one.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    path: String,
    head_only: bool,
    start: u64,
    end: Option<u64>,
}

fn parse(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?;
    let path = first.next()?.to_owned();
    if method != "GET" && method != "HEAD" {
        return None;
    }
    let mut start = 0;
    let mut end = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("range") {
            continue;
        }
        let range = value.trim().strip_prefix("bytes=")?;
        let (from, to) = range.split_once('-')?;
        start = from.trim().parse().ok()?;
        end = match to.trim() {
            "" => None,
            to => Some(to.parse().ok()?),
        };
    }
    Some(Request {
        path,
        head_only: method == "HEAD",
        start,
        end,
    })
}

async fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buf).await?;
        if read == 0 || head.len() + read > MAX_HEAD {
            return Ok(None);
        }
        head.extend_from_slice(&buf[..read]);
    }
    Ok(Some(String::from_utf8_lossy(&head).into_owned()))
}

async fn serve(
    mut stream: TcpStream,
    backend: &dyn Backend,
    key: &str,
    path: &str,
) -> std::io::Result<()> {
    // ffmpeg opens a new connection per seek, so one request per connection.
    let Some(head) = read_head(&mut stream).await? else {
        return Ok(());
    };
    let request = match parse(&head) {
        Some(request) if request.path == path => request,
        _ => return reply(&mut stream, "404 Not Found", &[]).await,
    };
    let length = |start: u64, end: Option<u64>| match end {
        Some(end) => (end + 1).saturating_sub(start).min(CHUNK),
        None => CHUNK,
    };
    let first = match backend
        .video(key, request.start, Some(length(request.start, request.end)))
        .await
    {
        Ok(chunk) => chunk,
        Err(error) => {
            tracing::debug!("video relay: kopuzd: {error}");
            return reply(&mut stream, "502 Bad Gateway", &[]).await;
        }
    };
    let Some(total) = first.total else {
        // Without a length there are no ranges to give; the first chunk is all.
        let headers = [
            ("Content-Type", first.content_type.clone()),
            ("Content-Length", first.bytes.len().to_string()),
        ];
        reply(&mut stream, "200 OK", &headers).await?;
        return stream.write_all(&first.bytes).await;
    };
    if request.start >= total {
        let headers = [("Content-Range", format!("bytes */{total}"))];
        return reply(&mut stream, "416 Range Not Satisfiable", &headers).await;
    }
    let last = request.end.unwrap_or(total - 1).min(total - 1);
    let headers = [
        ("Content-Type", first.content_type.clone()),
        ("Accept-Ranges", "bytes".to_owned()),
        ("Content-Length", (last + 1 - request.start).to_string()),
        (
            "Content-Range",
            format!("bytes {}-{last}/{total}", request.start),
        ),
    ];
    reply(&mut stream, "206 Partial Content", &headers).await?;
    if request.head_only {
        return Ok(());
    }
    let mut at = request.start;
    let mut chunk = first;
    loop {
        if chunk.bytes.is_empty() {
            return Ok(());
        }
        let take = chunk.bytes.len().min((last + 1 - at) as usize);
        stream.write_all(&chunk.bytes[..take]).await?;
        at += take as u64;
        if at > last {
            return Ok(());
        }
        chunk = match backend.video(key, at, Some(length(at, Some(last)))).await {
            Ok(chunk) => chunk,
            Err(error) => {
                tracing::debug!("video relay: kopuzd at {at}: {error}");
                return Ok(());
            }
        };
    }
}

async fn reply(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, String)],
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !headers.iter().any(|(name, _)| *name == "Content-Length") {
        head.push_str("Content-Length: 0\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_request_says_where_it_starts_and_ends() {
        let head = "GET /abc HTTP/1.1\r\nUser-Agent: Lavf\r\nRange: bytes=100-\r\n\r\n";
        assert_eq!(
            parse(head),
            Some(Request {
                path: "/abc".into(),
                head_only: false,
                start: 100,
                end: None,
            })
        );
        let head = "HEAD /abc HTTP/1.1\r\nrange: bytes=0-499\r\n\r\n";
        let request = parse(head).unwrap();
        assert!(request.head_only);
        assert_eq!((request.start, request.end), (0, Some(499)));
        assert_eq!(parse("POST /abc HTTP/1.1\r\n\r\n"), None);
    }
}
