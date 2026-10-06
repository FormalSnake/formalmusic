//! Opening a playlist for playback: enough of it to start right away, then
//! the rest page by page while it plays. Mixes never end, so they play from
//! the watch page's queue like the web app and go on the way radio does.

use formalmusic_api::{
    ApiError, BrowseTarget, Continuation, ContinuationPage, Header, Item, Page, Track,
};
use formalmusic_innertube::{Client, NextResult};
use std::collections::HashSet;

/// YouTube caps playlists at 5000 tracks, so a list that runs on past that
/// is a mix nothing marked as one.
const MAX_TRACKS: usize = 5000;

/// The InnerTube calls a playlist needs, so tests can stand in for YouTube.
pub trait Api {
    async fn browse_playlist(&self, playlist_id: &str) -> Result<Page, ApiError>;
    async fn continuation(&self, token: &Continuation) -> Result<ContinuationPage, ApiError>;
    async fn next(&self, video_id: Option<&str>, playlist_id: &str)
    -> Result<NextResult, ApiError>;
    async fn next_continuation(
        &self,
        playlist_id: &str,
        token: &Continuation,
    ) -> Result<NextResult, ApiError>;
}

impl Api for Client {
    async fn browse_playlist(&self, playlist_id: &str) -> Result<Page, ApiError> {
        self.browse(BrowseTarget::Playlist(playlist_id.to_owned()))
            .await
    }

    async fn continuation(&self, token: &Continuation) -> Result<ContinuationPage, ApiError> {
        Client::continuation(self, token).await
    }

    async fn next(
        &self,
        video_id: Option<&str>,
        playlist_id: &str,
    ) -> Result<NextResult, ApiError> {
        Client::next(self, video_id, Some(playlist_id)).await
    }

    async fn next_continuation(
        &self,
        playlist_id: &str,
        token: &Continuation,
    ) -> Result<NextResult, ApiError> {
        Client::next_continuation(self, playlist_id, token).await
    }
}

pub enum Opened {
    /// A list with an end, from `start`; `rest` brings what follows.
    List {
        tracks: Vec<Track>,
        start: usize,
        rest: Option<Rest>,
    },
    /// The first page of a mix's watch queue, extended like radio.
    Mix { next: NextResult, start: usize },
}

/// Opens `playlist_id` at `start`. `known` are the tracks the client's page
/// already shows, from the top of the list; they play without waiting.
pub async fn open(
    api: &impl Api,
    playlist_id: &str,
    known: Vec<Track>,
    start: usize,
) -> Result<Opened, ApiError> {
    if is_mix(playlist_id) {
        let video_id = known.get(start).map(|t| t.video_id.clone());
        return open_mix(api, playlist_id, video_id.as_deref()).await;
    }
    let mut rest = Rest::new(playlist_id, known.len());
    if !known.is_empty() {
        return Ok(Opened::List {
            tracks: known,
            start,
            rest: Some(rest),
        });
    }
    let mut tracks = Vec::new();
    while tracks.len() <= start {
        let Some(chunk) = rest.page(api).await? else {
            break;
        };
        if chunk.mix {
            let video_id = chunk.tracks.get(start).map(|t| t.video_id.clone());
            return open_mix(api, playlist_id, video_id.as_deref()).await;
        }
        tracks.extend(chunk.tracks);
    }
    Ok(Opened::List {
        tracks,
        start,
        rest: (!rest.done()).then_some(rest),
    })
}

/// The watch page for a mix, as the web app opens it from a row or the
/// Play button, starting at `video_id` when there is one.
async fn open_mix(
    api: &impl Api,
    playlist_id: &str,
    video_id: Option<&str>,
) -> Result<Opened, ApiError> {
    let next = api.next(video_id, playlist_id).await?;
    let start = video_id
        .and_then(|id| next.tracks.iter().position(|t| t.video_id == id))
        .unwrap_or(0);
    Ok(Opened::Mix { next, start })
}

/// `RD` lists (`RDTMAK`, `RDCLAK`, `RDAMVM`, `RDEM`, ...) are the ones the web
/// app plays from the watch page instead of a playlist page.
fn is_mix(playlist_id: &str) -> bool {
    playlist_id
        .strip_prefix("VL")
        .unwrap_or(playlist_id)
        .starts_with("RD")
}

/// The part of a playlist that has not been fetched yet.
pub struct Rest {
    playlist_id: String,
    cursor: Cursor,
    /// Tracks from the top of the list the queue already holds.
    skip: usize,
    fetched: usize,
    /// Pages of the up-next panel overlap.
    seen: HashSet<String>,
}

enum Cursor {
    Top,
    Browse(Continuation),
    Panel {
        panel_id: String,
        token: Continuation,
    },
    Done,
}

pub struct Chunk {
    pub tracks: Vec<Track>,
    /// The playlist page calls itself a mix.
    mix: bool,
}

impl Rest {
    fn new(playlist_id: &str, skip: usize) -> Self {
        Self {
            playlist_id: playlist_id.to_owned(),
            cursor: Cursor::Top,
            skip,
            fetched: 0,
            seen: HashSet::new(),
        }
    }

    fn done(&self) -> bool {
        matches!(self.cursor, Cursor::Done)
    }

    /// The next page, or `None` once the list is through.
    pub async fn page(&mut self, api: &impl Api) -> Result<Option<Chunk>, ApiError> {
        let mut chunk = match std::mem::replace(&mut self.cursor, Cursor::Done) {
            Cursor::Done => return Ok(None),
            Cursor::Top => self.top(api).await?,
            Cursor::Browse(token) => {
                let more = api.continuation(&token).await?;
                let tracks = tracks_of(more.items);
                if !tracks.is_empty()
                    && let Some(token) = more.continuation
                {
                    self.cursor = Cursor::Browse(token);
                }
                Chunk { tracks, mix: false }
            }
            Cursor::Panel { panel_id, token } => {
                let more = api.next_continuation(&panel_id, &token).await?;
                let tracks = self.fresh(more.tracks);
                if !tracks.is_empty()
                    && let Some(token) = more.continuation
                {
                    self.cursor = Cursor::Panel { panel_id, token };
                }
                Chunk { tracks, mix: false }
            }
        };
        self.fetched += chunk.tracks.len();
        if self.fetched >= MAX_TRACKS {
            let over = self.fetched - MAX_TRACKS;
            chunk.tracks.truncate(chunk.tracks.len() - over);
            self.cursor = Cursor::Done;
        }
        let known = self.skip.min(chunk.tracks.len());
        chunk.tracks.drain(..known);
        self.skip -= known;
        Ok(Some(chunk))
    }

    /// The playlist page, or for albums, which have none of their own
    /// (`OLAK5uy_...`), the up-next panel the web app shows for them.
    async fn top(&mut self, api: &impl Api) -> Result<Chunk, ApiError> {
        match api.browse_playlist(&self.playlist_id).await {
            Ok(page) => {
                let mix = matches!(&page.header, Some(Header::Detail { subtitle, .. })
                    if subtitle.first().is_some_and(|l| l.text == "Mix"));
                if let Some(list) = page.sections.into_iter().next() {
                    let tracks = tracks_of(list.items);
                    if !tracks.is_empty() {
                        if let Some(token) = list.continuation {
                            self.cursor = Cursor::Browse(token);
                        }
                        return Ok(Chunk { tracks, mix });
                    }
                }
            }
            Err(ApiError::Parse(_) | ApiError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let first = api.next(None, &self.playlist_id).await?;
        let panel_id = first
            .playlist_id
            .unwrap_or_else(|| self.playlist_id.clone());
        let tracks = self.fresh(first.tracks);
        if let Some(token) = first.continuation {
            self.cursor = Cursor::Panel { panel_id, token };
        }
        Ok(Chunk { tracks, mix: false })
    }

    fn fresh(&mut self, tracks: Vec<Track>) -> Vec<Track> {
        tracks
            .into_iter()
            .filter(|t| self.seen.insert(t.video_id.clone()))
            .collect()
    }
}

fn tracks_of(items: Vec<Item>) -> Vec<Track> {
    items
        .into_iter()
        .filter_map(|item| match item {
            Item::Track(track) => Some(track),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use formalmusic_api::{Link, Rating, Section, SectionLayout, TrackKind};
    use std::sync::Mutex;

    fn track(id: &str) -> Track {
        Track {
            video_id: id.into(),
            title: id.to_uppercase(),
            artists: Vec::new(),
            album: None,
            duration_ms: Some(180_000),
            thumbnails: Vec::new(),
            explicit: false,
            kind: TrackKind::Song,
            like: Rating::Indifferent,
            set_video_id: None,
            plays: None,
            feedback_token: None,
        }
    }

    fn tracks(ids: &str) -> Vec<Track> {
        ids.split(' ').map(track).collect()
    }

    fn ids(tracks: &[Track]) -> String {
        tracks
            .iter()
            .map(|t| t.video_id.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A playlist page in `pages`, each browse page but the last handing out
    /// a token for the next, and a watch queue of `queue`.
    #[derive(Default)]
    struct Fake {
        pages: Vec<&'static str>,
        subtitle: &'static str,
        queue: &'static str,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn page(&self, index: usize) -> (Vec<Item>, Option<Continuation>) {
            let items = tracks(self.pages[index])
                .into_iter()
                .map(Item::Track)
                .collect();
            let token = (index + 1 < self.pages.len()).then(|| Continuation(index.to_string()));
            (items, token)
        }
    }

    impl Api for Fake {
        async fn browse_playlist(&self, playlist_id: &str) -> Result<Page, ApiError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("browse {playlist_id}"));
            let (items, continuation) = self.page(0);
            Ok(Page {
                target: BrowseTarget::Playlist(playlist_id.into()),
                header: Some(Header::Detail {
                    title: "List".into(),
                    subtitle: vec![Link {
                        text: self.subtitle.into(),
                        target: None,
                    }],
                    second_subtitle: None,
                    description: None,
                    thumbnails: Vec::new(),
                    playlist_id: Some(playlist_id.into()),
                    editable: false,
                    saved: None,
                    privacy: None,
                }),
                chips: Vec::new(),
                sections: vec![Section {
                    title: None,
                    strapline: None,
                    layout: SectionLayout::List,
                    items,
                    more: None,
                    continuation,
                }],
                continuation: None,
            })
        }

        async fn continuation(&self, token: &Continuation) -> Result<ContinuationPage, ApiError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("continue {}", token.0));
            let (items, continuation) = self.page(token.0.parse::<usize>().unwrap() + 1);
            Ok(ContinuationPage {
                sections: Vec::new(),
                items,
                continuation,
            })
        }

        async fn next(
            &self,
            video_id: Option<&str>,
            playlist_id: &str,
        ) -> Result<NextResult, ApiError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("next {} {playlist_id}", video_id.unwrap_or("-")));
            Ok(NextResult {
                tracks: tracks(self.queue),
                playlist_id: Some(playlist_id.into()),
                continuation: Some(Continuation("radio".into())),
                ..NextResult::default()
            })
        }

        async fn next_continuation(
            &self,
            _: &str,
            _: &Continuation,
        ) -> Result<NextResult, ApiError> {
            unreachable!("mixes go on through the radio top-up")
        }
    }

    fn list(opened: Opened) -> (Vec<Track>, usize, Option<Rest>) {
        match opened {
            Opened::List {
                tracks,
                start,
                rest,
            } => (tracks, start, rest),
            Opened::Mix { .. } => panic!("opened as a mix"),
        }
    }

    #[tokio::test]
    async fn mixes_open_on_the_watch_page() {
        let fake = Fake {
            pages: vec!["a b c"],
            queue: "x y z",
            ..Fake::default()
        };
        for id in [
            "RDTMAK5uy_kset8DisdE7LSD4TNjEVvrKRTmG7a56sY",
            "VLRDCLAK5uy_abc",
            "RDAMVMdQw4w9WgXcQ",
            "RDEMabc",
        ] {
            let Opened::Mix { next, start } = open(&fake, id, Vec::new(), 0).await.unwrap() else {
                panic!("{id} did not open as a mix");
            };
            assert_eq!(ids(&next.tracks), "x y z");
            assert_eq!(start, 0);
        }
        assert!(fake.calls().iter().all(|c| c.starts_with("next -")));

        // A row click starts the watch queue at that row's track.
        let Opened::Mix { start, .. } = open(&fake, "RDTMAK5uy_x", tracks("a y"), 1).await.unwrap()
        else {
            panic!("not a mix");
        };
        assert_eq!(start, 1);
        assert_eq!(fake.calls().last().unwrap(), "next y RDTMAK5uy_x");

        // Other ids go to the playlist page, unless it calls itself a mix.
        let fake = Fake {
            pages: vec!["a b c"],
            subtitle: "Mix",
            queue: "x y z",
            ..Fake::default()
        };
        let opened = open(&fake, "PLsupermix", Vec::new(), 0).await.unwrap();
        assert!(matches!(opened, Opened::Mix { .. }));
        assert_eq!(fake.calls(), ["browse PLsupermix", "next a PLsupermix"]);
    }

    #[tokio::test]
    async fn first_page_plays_before_the_rest_loads() {
        let fake = Fake {
            pages: vec!["a b", "c d", "e"],
            subtitle: "Playlist",
            ..Fake::default()
        };
        let (tracks, start, rest) = list(open(&fake, "PL1", Vec::new(), 0).await.unwrap());
        assert_eq!((ids(&tracks).as_str(), start), ("a b", 0));
        assert_eq!(fake.calls(), ["browse PL1"]);

        let mut rest = rest.expect("more pages");
        assert_eq!(ids(&rest.page(&fake).await.unwrap().unwrap().tracks), "c d");
        assert_eq!(ids(&rest.page(&fake).await.unwrap().unwrap().tracks), "e");
        assert!(rest.page(&fake).await.unwrap().is_none());
        assert_eq!(fake.calls(), ["browse PL1", "continue 0", "continue 1"]);
    }

    #[tokio::test]
    async fn a_start_past_the_first_page_waits_only_for_its_page() {
        let fake = Fake {
            pages: vec!["a b", "c d", "e"],
            ..Fake::default()
        };
        let (tracks, start, rest) = list(open(&fake, "PL1", Vec::new(), 3).await.unwrap());
        assert_eq!((ids(&tracks).as_str(), start), ("a b c d", 3));
        let mut rest = rest.expect("one more page");
        assert_eq!(ids(&rest.page(&fake).await.unwrap().unwrap().tracks), "e");
    }

    #[tokio::test]
    async fn tracks_from_the_page_play_at_once_and_the_rest_follows() {
        let fake = Fake {
            pages: vec!["a b", "c d", "e"],
            ..Fake::default()
        };
        let (tracks, start, rest) = list(open(&fake, "PL1", tracks("a b c"), 2).await.unwrap());
        assert_eq!((ids(&tracks).as_str(), start), ("a b c", 2));
        assert!(fake.calls().is_empty());

        let mut rest = rest.expect("the rest");
        let mut more = Vec::new();
        while let Some(chunk) = rest.page(&fake).await.unwrap() {
            more.extend(chunk.tracks);
        }
        assert_eq!(ids(&more), "d e");
    }
}
