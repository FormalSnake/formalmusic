use crate::{Error, Result};
use reqwest::{Client, RequestBuilder, StatusCode};
use std::time::Duration;

pub(crate) const USER_AGENT: &str = "FormalMusic (https://github.com/FormalSnake/formalmusic)";

pub(crate) fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(5))
        .build()?)
}

/// Sends the request and reads the body. Transport failures, 5xx and 429 are
/// errors; any other status comes back to the caller, who decides whether a
/// 404 means "not found" for that endpoint.
pub(crate) async fn fetch(req: RequestBuilder, timeout: Duration) -> Result<(StatusCode, String)> {
    let res = req.timeout(timeout).send().await?;
    let status = res.status();
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        return Err(Error::Status {
            url: res.url().to_string(),
            status: status.as_u16(),
        });
    }
    Ok((status, res.text().await?))
}

/// The body of a 2xx answer; anything else is an error.
pub(crate) async fn text(req: RequestBuilder, timeout: Duration) -> Result<String> {
    let (status, body) = fetch(req, timeout).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(Error::Status {
            url: String::new(),
            status: status.as_u16(),
        })
    }
}
