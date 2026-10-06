pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// A server-side failure or rate limit. Client errors such as a 404 mean
    /// "not found" and never reach this.
    #[error("{url} answered {status}")]
    Status { url: String, status: u16 },
    #[error("cache: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected response from {0}")]
    Malformed(&'static str),
    #[error("lookup was cancelled")]
    Cancelled,
    #[error("apple music web player: {0}")]
    WebPlayer(&'static str),
}
