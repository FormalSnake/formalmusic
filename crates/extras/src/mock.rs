//! A tiny HTTP server for exercising the providers without the network.
//! `{base}` in a reply body is replaced with the server's own address.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub(crate) struct Route {
    prefix: String,
    /// Answered in order; the last one repeats.
    replies: Vec<(u16, Vec<u8>)>,
    served: AtomicUsize,
}

impl Route {
    /// Whether this route answers `prefix` or something under it, for tests
    /// that swap one route for another.
    pub(crate) fn matches(&self, prefix: &str) -> bool {
        self.prefix.starts_with(prefix)
    }
}

pub(crate) fn route(prefix: &str, status: u16, body: impl Into<Vec<u8>>) -> Route {
    sequence(prefix, vec![(status, body.into())])
}

pub(crate) fn sequence(prefix: &str, replies: Vec<(u16, Vec<u8>)>) -> Route {
    Route {
        prefix: prefix.to_owned(),
        replies,
        served: AtomicUsize::new(0),
    }
}

pub(crate) struct Mock {
    pub base: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl Mock {
    pub(crate) async fn start(routes: Vec<Route>) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes = Arc::new(routes);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        let served_base = base.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let (routes, log, base) =
                    (Arc::clone(&routes), Arc::clone(&log), served_base.clone());
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 2048];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&head).into_owned();
                    let target = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    log.lock().unwrap().push((target.clone(), head));
                    let (status, body) = match routes.iter().find(|r| target.starts_with(&r.prefix))
                    {
                        Some(r) => {
                            let i = r
                                .served
                                .fetch_add(1, Ordering::SeqCst)
                                .min(r.replies.len() - 1);
                            r.replies[i].clone()
                        }
                        None => (404, Vec::new()),
                    };
                    let body = match String::from_utf8(body) {
                        Ok(text) => text.replace("{base}", &base).into_bytes(),
                        Err(binary) => binary.into_bytes(),
                    };
                    let header = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        Mock { base, requests }
    }

    /// Request targets (path and query) in arrival order.
    pub(crate) fn targets(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(t, _)| t.clone())
            .collect()
    }

    /// The raw request heads whose target starts with `prefix`.
    pub(crate) fn heads(&self, prefix: &str) -> Vec<String> {
        let all = self.requests.lock().unwrap();
        all.iter()
            .filter(|(t, _)| t.starts_with(prefix))
            .map(|(_, h)| h.clone())
            .collect()
    }
}
