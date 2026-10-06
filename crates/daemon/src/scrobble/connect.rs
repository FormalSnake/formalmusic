//! The ListenBrainz user token from a browser profile signed in to
//! listenbrainz.org: the profile's cookies fetch the settings page, which
//! is where ListenBrainz shows a signed-in user their token.

use crate::signin::{BrowserSignIn, Cookie};
use formalmusic_api::ApiError;

const SETTINGS: &str = "https://listenbrainz.org/settings/";

/// The listenbrainz.org cookies as a `Cookie` header, once the profile has
/// a session there.
fn session_header(cookies: &[Cookie]) -> Option<String> {
    let ours: Vec<&Cookie> = cookies
        .iter()
        .filter(|c| {
            let host = c.domain.strip_prefix('.').unwrap_or(&c.domain);
            host == "listenbrainz.org"
        })
        .collect();
    ours.iter()
        .any(|c| c.name == "session" || c.name == "remember_token")
        .then(|| {
            ours.iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; ")
        })
}

pub async fn token_from_profile(
    signin: &BrowserSignIn,
    http: &reqwest::Client,
    browser: &str,
    profile: &str,
) -> Result<String, ApiError> {
    let header = signin
        .read_cookies(browser, profile, session_header)
        .await?
        .ok_or_else(|| {
            ApiError::BadRequest(
                "That profile is not signed in to listenbrainz.org. Sign in there, or paste your token instead."
                    .into(),
            )
        })?;
    // The settings page is a single-page app: a GET serves the shell and
    // the page posts to the same URL for its data. Either may carry it.
    for request in [http.post(SETTINGS), http.get(SETTINGS)] {
        let Ok(response) = request
            .header("Cookie", &header)
            .header("Accept", "application/json, text/html")
            .send()
            .await
        else {
            continue;
        };
        if let Ok(body) = response.text().await
            && let Some(token) = find_token(&body)
        {
            return Ok(token);
        }
    }
    Err(ApiError::BadRequest(
        "listenbrainz.org did not show a token for that profile. Paste it from listenbrainz.org/settings instead."
            .into(),
    ))
}

/// The UUID after an `auth_token` key, in JSON or in HTML-escaped JSON.
pub fn find_token(body: &str) -> Option<String> {
    let at = body.find("auth_token")?;
    let rest = &body[at + "auth_token".len()..];
    let window = &rest[..rest.len().min(64)];
    window
        .split(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
        .find(|run| is_uuid(run))
        .map(str::to_owned)
}

pub fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
        && parts
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(domain: &str, name: &str) -> Cookie {
        Cookie {
            domain: domain.into(),
            name: name.into(),
            value: "x".into(),
        }
    }

    #[test]
    fn needs_a_listenbrainz_session() {
        assert_eq!(session_header(&[cookie(".last.fm", "session")]), None);
        assert_eq!(session_header(&[cookie("listenbrainz.org", "_ga")]), None);
        assert_eq!(
            session_header(&[
                cookie("listenbrainz.org", "session"),
                cookie(".listenbrainz.org", "_ga"),
                cookie("musicbrainz.org", "session"),
            ]),
            Some("session=x; _ga=x".into())
        );
    }

    #[test]
    fn finds_the_token_in_json_and_escaped_html() {
        let token = "8f1c2a3b-4d5e-4f60-8a9b-0c1d2e3f4a5b";
        assert_eq!(
            find_token(&format!(
                r#"{{"user":{{"auth_token":"{token}","name":"a"}}}}"#
            )),
            Some(token.into())
        );
        assert_eq!(
            find_token(&format!("&#34;auth_token&#34;: &#34;{token}&#34;")),
            Some(token.into())
        );
        assert_eq!(find_token(r#"{"auth_token":null}"#), None);
        assert_eq!(find_token("<html>login</html>"), None);
    }
}
