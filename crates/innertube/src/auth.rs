use sha1::{Digest, Sha1};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const ORIGIN: &str = "https://music.youtube.com";

/// A signed-in session: the browser's cookie header and the brand account it
/// acts as, if any. The header follows the `Set-Cookie`s YouTube answers
/// with, as a browser's jar would; clones share it.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    cookie: Arc<Mutex<String>>,
    pub page_id: Option<String>,
}

impl Session {
    /// `None` when the cookies carry no `SAPISID`, which every signed-in
    /// Google session has.
    pub fn new(cookie: &str, page_id: Option<String>) -> Option<Self> {
        sapisid(cookie)?;
        Some(Self {
            cookie: Arc::new(Mutex::new(cookie.trim().to_owned())),
            page_id,
        })
    }

    pub fn cookie(&self) -> String {
        self.cookie
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// `SAPISIDHASH <ts>_<sha1("<ts> <SAPISID> <origin>")>`, the header the web
    /// app derives from its cookies on every API call.
    pub fn authorization(&self, unix_secs: u64) -> String {
        let cookie = self.cookie();
        let sapisid = sapisid(&cookie).unwrap_or_default();
        let digest = Sha1::digest(format!("{unix_secs} {sapisid} {ORIGIN}").as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        format!("SAPISIDHASH {unix_secs}_{hex}")
    }

    /// Takes in the `Set-Cookie`s of one response.
    pub fn absorb(&self, response: &reqwest::Response) {
        let lines = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok());
        let mut cookie = self.cookie.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(merged) = merge_set_cookies(&cookie, lines, current_year()) {
            *cookie = merged;
        }
    }

    /// Sets each `(name, value)` in the header, adding the ones it lacks.
    pub fn merge(&self, pairs: &[(String, String)]) {
        let mut cookie = self.cookie.lock().unwrap_or_else(|e| e.into_inner());
        let mut jar = parse_header(&cookie);
        for (name, value) in pairs {
            set(&mut jar, name, value);
        }
        *cookie = join(&jar);
    }
}

fn sapisid(cookie: &str) -> Option<&str> {
    cookie_value(cookie, "SAPISID").or_else(|| cookie_value(cookie, "__Secure-3PAPISID"))
}

fn parse_header(header: &str) -> Vec<(String, String)> {
    header
        .split(';')
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (!name.is_empty()).then(|| (name.to_owned(), value.to_owned()))
        })
        .collect()
}

fn join(jar: &[(String, String)]) -> String {
    jar.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn set(jar: &mut Vec<(String, String)>, name: &str, value: &str) {
    match jar.iter_mut().find(|(n, _)| n == name) {
        Some(entry) => entry.1 = value.to_owned(),
        None => jar.push((name.to_owned(), value.to_owned())),
    }
}

fn current_year() -> u32 {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or_default();
    // Close enough for comparing against the year of an `Expires` date.
    1970 + (days * 400 / 146_097) as u32
}

/// `header` with the youtube.com cookies of `set_cookies` applied, or `None`
/// when nothing changed. YouTube rotates `SIDCC` and friends on the way and
/// deletes a cookie by setting it with an expiry years in the past.
fn merge_set_cookies<'a>(
    header: &str,
    set_cookies: impl Iterator<Item = &'a str>,
    year: u32,
) -> Option<String> {
    let mut jar = parse_header(header);
    let before = jar.clone();
    for line in set_cookies {
        let mut parts = line.split(';');
        let Some((name, value)) = parts.next().and_then(|p| p.trim().split_once('=')) else {
            continue;
        };
        let (mut ours, mut expired) = (true, false);
        for attr in parts {
            let (key, val) = attr.trim().split_once('=').unwrap_or((attr.trim(), ""));
            match key.to_ascii_lowercase().as_str() {
                "domain" => ours = val.trim_start_matches('.').ends_with("youtube.com"),
                "max-age" => expired |= val.trim().parse::<i64>().is_ok_and(|age| age <= 0),
                "expires" => {
                    expired |= val
                        .split([' ', '-'])
                        .find_map(|word| word.parse::<u32>().ok().filter(|y| *y > 1000))
                        .is_some_and(|y| y < year)
                }
                _ => {}
            }
        }
        let name = name.trim();
        if !ours || name.is_empty() {
            continue;
        }
        if expired {
            jar.retain(|(n, _)| n != name);
        } else {
            set(&mut jar, name, value.trim());
        }
    }
    (jar != before).then(|| join(&jar))
}

pub(crate) fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

/// A `Cookie` header from a Netscape cookie file (the format yt-dlp and
/// browser export extensions write), keeping the youtube.com cookies.
pub fn cookie_header_from_netscape(file: &str) -> String {
    file.lines()
        .filter_map(|line| {
            // curl marks HttpOnly cookies by prefixing the domain, which
            // otherwise reads as a comment.
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') {
                return None;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [domain, _, _, _, _, name, value] = fields[..] else {
                return None;
            };
            domain
                .trim_start_matches('.')
                .ends_with("youtube.com")
                .then(|| format!("{name}={}", value.trim_end()))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_netscape_cookie_files() {
        let file = "# Netscape HTTP Cookie File\n\
            .youtube.com\tTRUE\t/\tTRUE\t1893456000\tSAPISID\tabc\n\
            #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1893456000\tSID\tdef\n\
            .google.com\tTRUE\t/\tTRUE\t1893456000\tNID\tzzz\n\
            \n";
        assert_eq!(cookie_header_from_netscape(file), "SAPISID=abc; SID=def");
    }

    #[test]
    fn finds_sapisid_among_other_cookies() {
        let session = Session::new("HSID=a; SAPISID=abc/def; __Secure-3PAPISID=zzz", None).unwrap();
        assert_eq!(sapisid(&session.cookie()), Some("abc/def"));
        assert!(Session::new("HSID=a; SID=b", None).is_none());
        assert!(Session::new("__Secure-3PAPISID=zzz", None).is_some());
    }

    #[test]
    fn follows_set_cookie() {
        let header = "SID=a; SIDCC=old; __Secure-3PSIDCC=old3; YSC=y";
        let merged = merge_set_cookies(
            header,
            [
                "SIDCC=new; expires=Wed, 06-Oct-2027 18:00:00 GMT; path=/; domain=.youtube.com; priority=high",
                "__Secure-3PSIDCC=new3; Path=/; Domain=.youtube.com; Secure; HttpOnly",
                "YSC=; Domain=.youtube.com; Path=/; Expires=Thu, 01-Jan-1970 00:00:01 GMT",
                "NID=g; Domain=.google.com; Path=/",
                "VISITOR_INFO1_LIVE=v; Domain=.youtube.com; Max-Age=15552000",
            ]
            .into_iter(),
            2026,
        );
        assert_eq!(
            merged.as_deref(),
            Some("SID=a; SIDCC=new; __Secure-3PSIDCC=new3; VISITOR_INFO1_LIVE=v")
        );
        assert_eq!(
            merge_set_cookies("SID=a", ["SID=a; Path=/"].into_iter(), 2026),
            None
        );
        assert!((2026..2100).contains(&current_year()));
    }

    #[test]
    fn hash_matches_the_web_app() {
        let session = Session::new("SAPISID=abc/def", None).unwrap();
        assert_eq!(
            session.authorization(1_700_000_000),
            "SAPISIDHASH 1700000000_523b01f033dd8ed738175261f406736fe239c73d"
        );
    }
}
