use sha1::{Digest, Sha1};

pub(crate) const ORIGIN: &str = "https://music.youtube.com";

/// A signed-in session: the browser's cookie header and the brand account it
/// acts as, if any.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    pub cookie: String,
    sapisid: String,
    pub page_id: Option<String>,
}

impl Session {
    /// `None` when the cookies carry no `SAPISID`, which every signed-in
    /// Google session has.
    pub fn new(cookie: &str, page_id: Option<String>) -> Option<Self> {
        let sapisid = cookie_value(cookie, "SAPISID")
            .or_else(|| cookie_value(cookie, "__Secure-3PAPISID"))?;
        Some(Self {
            cookie: cookie.trim().to_owned(),
            sapisid: sapisid.to_owned(),
            page_id,
        })
    }

    /// `SAPISIDHASH <ts>_<sha1("<ts> <SAPISID> <origin>")>`, the header the web
    /// app derives from its cookies on every API call.
    pub fn authorization(&self, unix_secs: u64) -> String {
        let digest = Sha1::digest(format!("{unix_secs} {} {ORIGIN}", self.sapisid).as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        format!("SAPISIDHASH {unix_secs}_{hex}")
    }
}

pub(crate) fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sapisid_among_other_cookies() {
        let session = Session::new("HSID=a; SAPISID=abc/def; __Secure-3PAPISID=zzz", None).unwrap();
        assert_eq!(session.sapisid, "abc/def");
        assert!(Session::new("HSID=a; SID=b", None).is_none());
        assert!(Session::new("__Secure-3PAPISID=zzz", None).is_some());
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
