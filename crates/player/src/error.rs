use std::io;

/// Why a track could not be opened or stopped playing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlayerError {
    /// googlevideo answered 403 or 410: the signed URL expired or was never
    /// valid for this client. Resolve the track again and load it at the last
    /// position.
    #[error("stream url expired, resolve it again")]
    Expired,
    #[error("http status {0}")]
    Http(u16),
    #[error("network: {0}")]
    Network(String),
    #[error("unsupported stream: {0}")]
    Unsupported(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("audio output: {0}")]
    Output(String),
}

impl PlayerError {
    /// Recovers the error a [`crate::fetch`] reader wrapped into an
    /// `io::Error` on its way through symphonia.
    pub(crate) fn from_io(err: &io::Error) -> Self {
        match err
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<PlayerError>())
        {
            Some(inner) => inner.clone(),
            None => PlayerError::Network(err.to_string()),
        }
    }
}

impl From<symphonia::core::errors::Error> for PlayerError {
    fn from(err: symphonia::core::errors::Error) -> Self {
        use symphonia::core::errors::Error;
        match err {
            Error::IoError(err) => PlayerError::from_io(&err),
            Error::Unsupported(what) => PlayerError::Unsupported(what.to_owned()),
            other => PlayerError::Decode(other.to_string()),
        }
    }
}
