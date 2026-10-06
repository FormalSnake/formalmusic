//! Line framing shared by the daemon and its clients.

use serde::{Serialize, de::DeserializeOwned};
use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Lines past this are dropped as garbage rather than buffered forever. A full
/// playlist page with a few thousand tracks stays well under it.
pub const MAX_LINE: usize = 64 * 1024 * 1024;

pub async fn write<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}

/// `Ok(None)` on a clean end of stream.
pub async fn read<R: AsyncBufRead + Unpin, T: DeserializeOwned>(
    r: &mut R,
    buf: &mut String,
) -> io::Result<Option<T>> {
    buf.clear();
    let n = r.read_line(buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
    }
    serde_json::from_str(buf.trim_end())
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
