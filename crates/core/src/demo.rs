//! `FORMALMUSIC_DEMO=1`: a made-up catalog served without a daemon, for
//! screenshots, tests and working on the UI with no YouTube account.
//!
//! The pages are hand built in YouTube Music's shapes (Home with chips and
//! shelves, album, artist, a 1000 track playlist), with a player simulation
//! that moves the position on and the queue along.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::{Notify, mpsc};
use tokio::task::AbortHandle;

use crate::backend::{Backend, BackendKind, ClientError, ConnectionStatus, Control, Event, Result};
use crate::model::*;
use crate::settings::Settings;

const TICK: std::time::Duration = std::time::Duration::from_millis(250);
const LONG_PLAYLIST: &str = "PLdemo-late-night";
/// An artist's "see all songs" page is the artist's id with this after it.
const SONGS_SHELF: &str = "-songs";
const PLAYLIST_PAGE: usize = 100;

pub struct Album {
    pub browse_id: String,
    pub playlist_id: String,
    pub title: String,
    pub album_type: &'static str,
    pub artist: usize,
    pub year: u32,
    pub tracks: Vec<Track>,
}

pub struct Artist {
    pub browse_id: String,
    pub name: &'static str,
    pub subscribers: &'static str,
}

pub struct Playlist {
    pub playlist_id: String,
    pub title: &'static str,
    pub owned: bool,
    pub tracks: Vec<Track>,
}

pub struct Catalog {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub playlists: Vec<Playlist>,
}

const ARTISTS: [(&str, &str); 8] = [
    ("Lumen Atlas", "1.2M subscribers"),
    ("Harbor Lights", "640K subscribers"),
    ("Nadia Reyes", "2.4M subscribers"),
    ("The Quiet Fields", "318K subscribers"),
    ("Kofi Mensah", "905K subscribers"),
    ("Saltwater Choir", "122K subscribers"),
    ("Mira Okafor", "1.8M subscribers"),
    ("Velvet Transit", "477K subscribers"),
];

const ALBUMS: [&str; 20] = [
    "Low Tide Radio",
    "Glass Orchard",
    "Northbound",
    "Paper Satellites",
    "Slow Bloom",
    "Copper Hours",
    "Half Light",
    "Signal Fires",
    "Open Water",
    "The Long Weekend",
    "Neon Cathedral",
    "Fieldnotes",
    "Afterglow Avenue",
    "Second Summer",
    "Static & Honey",
    "Lanterns",
    "Velour",
    "Undertow",
    "Night Ferry",
    "Weather Systems",
];

const SONGS: [&str; 48] = [
    "Golden Hour Again",
    "Small Hours",
    "Parallel",
    "Driftwood",
    "Sodium Lights",
    "Catch the Last Train",
    "Every Window",
    "Saltwater",
    "Wild Geese",
    "Carry the Weight",
    "Sunday Static",
    "Overpass",
    "Ultraviolet",
    "Palms Up",
    "Cold Coffee",
    "Satellite Heart",
    "Undertow",
    "Still Life",
    "Hand in the River",
    "Glasshouse",
    "Runaway Bay",
    "Low Light",
    "Little Fires",
    "Northern Line",
    "Weightless",
    "Fault Lines",
    "Postcards",
    "Summer Ghosts",
    "Tidal",
    "Over the Hill",
    "Polaroid",
    "Kerosene",
    "Back Roads",
    "Ferris Wheel",
    "Constellations",
    "Hollow",
    "Daybreak",
    "Mercury",
    "Lost & Found",
    "Bright Side",
    "Eastbound",
    "Velvet Morning",
    "Afterimage",
    "Paper Cuts",
    "The Garden",
    "Slow Motion",
    "Heatwave",
    "Homecoming",
];

const LYRICS: [&str; 16] = [
    "I kept the porch light on for you",
    "Long after the street went quiet",
    "Every car that passed was almost you",
    "And I almost didn't mind it",
    "We were younger than the summer then",
    "Counting stars along the overpass",
    "Say the word and I'll be there again",
    "Holding on to what was meant to last",
    "Oh, the radio is playing our song",
    "Turn it up and let the windows down",
    "We don't have to know where we belong",
    "Just as long as we keep moving now",
    "Light it up, light it up",
    "Till the morning finds us here",
    "Light it up, light it up",
    "Till there's nothing left to fear",
];

const CHIPS: [&str; 10] = [
    "Energize",
    "Relax",
    "Feel good",
    "Workout",
    "Commute",
    "Focus",
    "Party",
    "Romance",
    "Sad",
    "Sleep",
];

const MOODS: [(&str, u32); 12] = [
    ("Chill", 0xff2d6a4f),
    ("Commute", 0xff9d4edd),
    ("Energy boosters", 0xffe76f51),
    ("Feel good", 0xfff4a261),
    ("Focus", 0xff264653),
    ("Party", 0xffd62828),
    ("Romance", 0xffc9184a),
    ("Sleep", 0xff3a0ca3),
    ("Workout", 0xffff7b00),
    ("Indie & alternative", 0xff588157),
    ("Jazz", 0xffbc6c25),
    ("Soul", 0xff6d597a),
];

fn art(seed: &str) -> Option<Art> {
    Some(Art {
        kind: ArtKind::Demo,
        id: seed.to_owned(),
        version: 0,
    })
}

/// The same pseudo-random sequence on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(build_catalog)
}

fn build_catalog() -> Catalog {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let artists: Vec<Artist> = ARTISTS
        .iter()
        .enumerate()
        .map(|(n, (name, subscribers))| Artist {
            browse_id: format!("UCdemo-artist-{n}"),
            name,
            subscribers,
        })
        .collect();
    let mut albums = Vec::new();
    for (n, title) in ALBUMS.iter().enumerate() {
        let artist = n % artists.len();
        let single = n % 5 == 4;
        let count = if single { 2 } else { 8 + rng.below(5) as usize };
        let browse_id = format!("MPREdemo-{n}");
        let tracks = (0..count)
            .map(|index| {
                let song = if single && index == 0 {
                    title
                } else {
                    SONGS[rng.below(SONGS.len() as u64) as usize]
                };
                let key = format!("demo-{n}-{index}");
                Track {
                    key: key.clone(),
                    title: song.to_string(),
                    artists: vec![Link {
                        text: artists[artist].name.into(),
                        target: Some(BrowseTarget::Artist(artists[artist].browse_id.clone())),
                    }],
                    album: Some(Link {
                        text: title.to_string(),
                        target: Some(BrowseTarget::Album(browse_id.clone())),
                    }),
                    duration_ms: Some(150_000 + rng.below(160_000)),
                    art: art(&format!("album-{n}")),
                    explicit: rng.below(7) == 0,
                    kind: TrackKind::Song,
                    plays: Some(format!("{}M plays", 1 + rng.below(90))),
                    actions: Actions {
                        rate_ref: Some(key),
                        rating: Some(Rating::Indifferent),
                        save_ref: Some(format!("demo-save-{n}-{index}")),
                        saved: Some(index % 3 == 0),
                        ..Actions::default()
                    },
                }
            })
            .collect();
        albums.push(Album {
            browse_id,
            playlist_id: format!("OLAKdemo-{n}"),
            title: title.to_string(),
            album_type: if single {
                "Single"
            } else if n % 7 == 3 {
                "EP"
            } else {
                "Album"
            },
            artist,
            year: 2019 + (n as u32 % 8),
            tracks,
        });
    }
    let every: Vec<&Track> = albums
        .iter()
        .flat_map(|album| album.tracks.iter())
        .collect();
    let pick = |count: usize, rng: &mut Rng| -> Vec<Track> {
        (0..count)
            .map(|_| every[rng.below(every.len() as u64) as usize].clone())
            .collect()
    };
    let playlists = vec![
        Playlist {
            playlist_id: "LM".into(),
            title: "Liked music",
            owned: false,
            tracks: pick(64, &mut rng),
        },
        Playlist {
            playlist_id: LONG_PLAYLIST.into(),
            title: "Late night drive",
            owned: true,
            tracks: pick(1000, &mut rng),
        },
        Playlist {
            playlist_id: "PLdemo-run".into(),
            title: "Morning run",
            owned: true,
            tracks: pick(32, &mut rng),
        },
        Playlist {
            playlist_id: "PLdemo-focus".into(),
            title: "Deep focus",
            owned: true,
            tracks: pick(48, &mut rng),
        },
        Playlist {
            playlist_id: "PLdemo-road".into(),
            title: "Road trip 2026",
            owned: true,
            tracks: pick(27, &mut rng),
        },
        Playlist {
            playlist_id: "PLdemo-saved".into(),
            title: "Saved for later",
            owned: true,
            tracks: pick(12, &mut rng),
        },
        Playlist {
            playlist_id: "RDdemo-mix-1".into(),
            title: "My supermix",
            owned: false,
            tracks: pick(50, &mut rng),
        },
        Playlist {
            playlist_id: "RDdemo-mix-2".into(),
            title: "Discover mix",
            owned: false,
            tracks: pick(50, &mut rng),
        },
    ];
    Catalog {
        artists,
        albums,
        playlists,
    }
}

fn artist_link(catalog: &Catalog, artist: usize) -> Link {
    Link {
        text: catalog.artists[artist].name.into(),
        target: Some(BrowseTarget::Artist(
            catalog.artists[artist].browse_id.clone(),
        )),
    }
}

fn album_item(catalog: &Catalog, n: usize) -> Item {
    let album = &catalog.albums[n];
    Item::Album {
        browse_id: album.browse_id.clone(),
        title: album.title.clone(),
        album_type: Some(album.album_type.into()),
        artists: vec![artist_link(catalog, album.artist)],
        year: Some(album.year.to_string()),
        art: art(&format!("album-{n}")),
        explicit: album.tracks.iter().any(|track| track.explicit),
        actions: Actions {
            save_ref: Some(album.playlist_id.clone()),
            saved: Some(n.is_multiple_of(3)),
            ..Actions::default()
        },
    }
}

fn artist_item(catalog: &Catalog, n: usize) -> Item {
    let artist = &catalog.artists[n];
    Item::Artist {
        browse_id: artist.browse_id.clone(),
        name: artist.name.into(),
        subtitle: Some(artist.subscribers.into()),
        art: art(&format!("artist-{n}")),
        actions: Actions {
            follow_ref: Some(artist.browse_id.clone()),
            followed: Some(n.is_multiple_of(2)),
            ..Actions::default()
        },
    }
}

fn playlist_item(playlist: &Playlist) -> Item {
    let owner = if playlist.owned {
        "You"
    } else {
        "YouTube Music"
    };
    Item::Playlist {
        playlist_id: playlist.playlist_id.clone(),
        title: playlist.title.into(),
        subtitle: Some(format!(
            "Playlist \u{2022} {owner} \u{2022} {} songs",
            playlist.tracks.len()
        )),
        art: art(&format!("playlist-{}", playlist.playlist_id)),
        actions: Actions {
            save_ref: (!playlist.owned).then(|| playlist.playlist_id.clone()),
            saved: (!playlist.owned).then_some(true),
            ..Actions::default()
        },
    }
}

fn section(title: &str, layout: SectionLayout, items: Vec<Item>) -> Section {
    Section {
        title: Some(title.into()),
        strapline: None,
        layout,
        items,
        more: None,
        continuation: None,
        ..Default::default()
    }
}

fn tracks_from(seed: u64, count: usize) -> Vec<Track> {
    let catalog = catalog();
    let mut rng = Rng(seed | 1);
    let every: Vec<&Track> = catalog
        .albums
        .iter()
        .flat_map(|album| album.tracks.iter())
        .collect();
    (0..count)
        .map(|_| every[rng.below(every.len() as u64) as usize].clone())
        .collect()
}

fn home(chip: Option<&str>) -> Page {
    let catalog = catalog();
    let seed = chip.map_or(7, |chip| chip.len() as u64 * 31);
    let chips = CHIPS
        .iter()
        .map(|title| Chip {
            title: (*title).into(),
            id: format!("chip-{title}"),
            selected: chip == Some(*title),
        })
        .collect();
    let mut quick = section(
        "Quick picks",
        SectionLayout::TrackGrid,
        tracks_from(seed, 16).into_iter().map(Item::Track).collect(),
    );
    quick.strapline = Some("Start radio from a song".into());
    let mut again = section(
        "Listen again",
        SectionLayout::Carousel,
        vec![
            album_item(catalog, 2),
            artist_item(catalog, 2),
            playlist_item(&catalog.playlists[1]),
            album_item(catalog, 9),
            album_item(catalog, 12),
            artist_item(catalog, 6),
            album_item(catalog, 5),
            playlist_item(&catalog.playlists[2]),
            album_item(catalog, 17),
            album_item(catalog, 0),
        ],
    );
    again.strapline = Some("Alex Rivera".into());
    let mixed = section(
        "Mixed for you",
        SectionLayout::Carousel,
        catalog
            .playlists
            .iter()
            .rev()
            .take(6)
            .map(playlist_item)
            .collect(),
    );
    let releases = section(
        "New releases",
        SectionLayout::Carousel,
        (0..catalog.albums.len())
            .rev()
            .step_by(2)
            .map(|n| album_item(catalog, n))
            .collect(),
    );
    let mut similar = section(
        "Similar to Nadia Reyes",
        SectionLayout::Carousel,
        (0..catalog.artists.len())
            .map(|n| artist_item(catalog, (n + 3) % 8))
            .collect(),
    );
    similar.strapline = Some("Recommended artists".into());
    Page {
        target: chip.map_or(BrowseTarget::Home, |chip| {
            BrowseTarget::HomeChip(format!("chip-{chip}"))
        }),
        header: None,
        chips,
        sections: vec![quick, again, mixed, releases, similar],
        continuation: Some(Continuation("home-1".into())),
    }
}

fn home_more(n: usize) -> ContinuationPage {
    let catalog = catalog();
    let sections = match n {
        1 => vec![
            section(
                "From the community",
                SectionLayout::Carousel,
                catalog.playlists.iter().map(playlist_item).collect(),
            ),
            section("Moods and genres", SectionLayout::Grid, moods()),
        ],
        2 => vec![
            section(
                "Albums for you",
                SectionLayout::Carousel,
                (0..catalog.albums.len())
                    .map(|n| album_item(catalog, (n * 7) % 20))
                    .collect(),
            ),
            section(
                "Trending songs",
                SectionLayout::TrackGrid,
                tracks_from(99, 12).into_iter().map(Item::Track).collect(),
            ),
        ],
        _ => vec![section(
            "Throwback jams",
            SectionLayout::Carousel,
            (0..10)
                .map(|n| album_item(catalog, (n * 3 + 1) % 20))
                .collect(),
        )],
    };
    let continuation = (n < 3).then(|| Continuation(format!("home-{}", n + 1)));
    ContinuationPage {
        sections,
        items: Vec::new(),
        continuation,
    }
}

fn moods() -> Vec<Item> {
    MOODS
        .iter()
        .map(|(title, color)| Item::Mood {
            title: (*title).into(),
            id: format!("mood-{title}"),
            color: Some(color & 0xffffff),
        })
        .collect()
}

fn shortcuts() -> Vec<Item> {
    [
        (
            "New releases",
            BrowseTarget::NewReleases,
            "MUSIC_NEW_RELEASE",
        ),
        ("Charts", BrowseTarget::Charts, "TRENDING_UP"),
        (
            "Moods and genres",
            BrowseTarget::MoodsAndGenres,
            "STICKER_EMOTICON",
        ),
    ]
    .into_iter()
    .map(|(title, target, icon)| Item::Shortcut {
        title: title.into(),
        target,
        icon: Some(icon.into()),
    })
    .collect()
}

fn explore() -> Page {
    let catalog = catalog();
    Page {
        target: BrowseTarget::Explore,
        header: None,
        chips: Vec::new(),
        sections: vec![
            Section {
                title: None,
                ..section("", SectionLayout::Grid, shortcuts())
            },
            section(
                "New albums and singles",
                SectionLayout::Carousel,
                (0..catalog.albums.len())
                    .map(|n| album_item(catalog, (n * 3) % 20))
                    .collect(),
            ),
            section("Moods and genres", SectionLayout::Grid, moods()),
            section(
                "Trending",
                SectionLayout::TrackGrid,
                tracks_from(31, 16).into_iter().map(Item::Track).collect(),
            ),
            section(
                "Top artists",
                SectionLayout::Carousel,
                (0..catalog.artists.len())
                    .map(|n| artist_item(catalog, n))
                    .collect(),
            ),
        ],
        continuation: None,
    }
}

fn library(tab: LibraryTab) -> Page {
    let catalog = catalog();
    let sections = match tab {
        LibraryTab::Playlists => vec![section(
            "Playlists",
            SectionLayout::Grid,
            catalog.playlists.iter().map(playlist_item).collect(),
        )],
        LibraryTab::Albums => vec![section(
            "Albums",
            SectionLayout::Grid,
            (0..catalog.albums.len())
                .map(|n| album_item(catalog, n))
                .collect(),
        )],
        LibraryTab::Artists | LibraryTab::Subscriptions => vec![section(
            "Artists",
            SectionLayout::Grid,
            (0..catalog.artists.len())
                .map(|n| artist_item(catalog, n))
                .collect(),
        )],
        _ => vec![section(
            "Songs",
            SectionLayout::List,
            catalog.playlists[0]
                .tracks
                .iter()
                .cloned()
                .map(Item::Track)
                .collect(),
        )],
    };
    let chips = ["Playlists", "Songs", "Albums", "Artists"]
        .iter()
        .map(|title| Chip {
            title: (*title).into(),
            id: format!("library-{}", title.to_lowercase()),
            selected: format!("{tab:?}") == *title,
        })
        .collect();
    Page {
        target: BrowseTarget::Library(tab),
        header: Some(Header::Title {
            title: "Library".into(),
        }),
        chips,
        sections,
        continuation: None,
    }
}

fn minutes(tracks: &[Track]) -> String {
    let total: u64 = tracks.iter().filter_map(|track| track.duration_ms).sum();
    let minutes = total / 60_000;
    if minutes >= 60 {
        format!("{} hr {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} minutes")
    }
}

fn album_page(browse_id: &str) -> Option<Page> {
    let catalog = catalog();
    let n = catalog
        .albums
        .iter()
        .position(|album| album.browse_id == browse_id)?;
    let album = &catalog.albums[n];
    let more = (0..catalog.albums.len())
        .filter(|other| *other != n && catalog.albums[*other].artist == album.artist)
        .map(|other| album_item(catalog, other))
        .collect();
    Some(Page {
        target: BrowseTarget::Album(browse_id.into()),
        header: Some(Header::Detail {
            title: album.title.clone(),
            subtitle: vec![
                Link {
                    text: album.album_type.into(),
                    target: None,
                },
                artist_link(catalog, album.artist),
                Link {
                    text: album.year.to_string(),
                    target: None,
                },
            ],
            second_subtitle: Some(format!(
                "{} songs \u{2022} {}",
                album.tracks.len(),
                minutes(&album.tracks)
            )),
            description: Some(format!(
                "{} recorded {} over one winter in a borrowed house by the sea.",
                catalog.artists[album.artist].name, album.title
            )),
            art: art(&format!("album-{n}")),
            play: Some(PlaySource::Page {
                target: BrowseTarget::Album(browse_id.into()),
                tracks: Vec::new(),
            }),
            editable: false,
            privacy: None,
            actions: Actions {
                save_ref: Some(album.playlist_id.clone()),
                saved: Some(n.is_multiple_of(3)),
                ..Actions::default()
            },
        }),
        chips: Vec::new(),
        sections: vec![
            Section {
                title: None,
                strapline: None,
                layout: SectionLayout::List,
                items: album.tracks.iter().cloned().map(Item::Track).collect(),
                more: None,
                continuation: None,
                ..Default::default()
            },
            section(
                &format!("More by {}", catalog.artists[album.artist].name),
                SectionLayout::Carousel,
                more,
            ),
        ],
        continuation: None,
    })
}

fn artist_page(browse_id: &str) -> Option<Page> {
    let catalog = catalog();
    let n = catalog
        .artists
        .iter()
        .position(|artist| artist.browse_id == browse_id)?;
    let artist = &catalog.artists[n];
    let own: Vec<usize> = (0..catalog.albums.len())
        .filter(|album| catalog.albums[*album].artist == n)
        .collect();
    let top: Vec<Item> = own
        .iter()
        .flat_map(|album| catalog.albums[*album].tracks.iter().take(3))
        .take(5)
        .cloned()
        .map(Item::Track)
        .collect();
    let albums = own
        .iter()
        .filter(|album| catalog.albums[**album].album_type != "Single")
        .map(|album| album_item(catalog, *album))
        .collect();
    let singles = own
        .iter()
        .filter(|album| catalog.albums[**album].album_type == "Single")
        .map(|album| album_item(catalog, *album))
        .collect();
    let fans = (1..catalog.artists.len())
        .map(|step| artist_item(catalog, (n + step) % catalog.artists.len()))
        .collect();
    let first = top.iter().find_map(|item| match item {
        Item::Track(track) => Some(track.key.clone()),
        _ => None,
    });
    let mut top_songs = section("Top songs", SectionLayout::List, top);
    top_songs.more = Some(BrowseTarget::Page(format!("{browse_id}{SONGS_SHELF}")));
    Some(Page {
        target: BrowseTarget::Artist(browse_id.into()),
        header: Some(Header::Artist {
            name: artist.name.into(),
            description: Some(format!(
                "{} writes songs about leaving and coming back, recorded mostly live with the same four players since the first record.",
                artist.name
            )),
            art: art(&format!("artist-{n}")),
            subscribers: Some(artist.subscribers.into()),
            monthly_listeners: Some(format!("{}.{}M monthly audience", 1 + n % 4, n % 10)),
            shuffle: Some(PlaySource::Artist {
                key: browse_id.into(),
            }),
            radio: first.map(|key| PlaySource::Radio { key }),
            actions: Actions {
                follow_ref: Some(browse_id.into()),
                followed: Some(n.is_multiple_of(2)),
                ..Actions::default()
            },
        }),
        chips: Vec::new(),
        sections: vec![
            top_songs,
            section("Albums", SectionLayout::Carousel, albums),
            section("Singles", SectionLayout::Carousel, singles),
            section("Fans might also like", SectionLayout::Carousel, fans),
        ],
        continuation: None,
    })
}

fn playlist_page(playlist_id: &str) -> Option<Page> {
    let catalog = catalog();
    let playlist = catalog
        .playlists
        .iter()
        .find(|playlist| playlist.playlist_id == playlist_id)?;
    let first: Vec<Item> = playlist
        .tracks
        .iter()
        .take(PLAYLIST_PAGE)
        .cloned()
        .map(Item::Track)
        .collect();
    let continuation = (playlist.tracks.len() > PLAYLIST_PAGE)
        .then(|| Continuation(format!("playlist:{playlist_id}:{PLAYLIST_PAGE}")));
    Some(Page {
        target: BrowseTarget::Playlist(playlist_id.into()),
        header: Some(Header::Detail {
            title: playlist.title.into(),
            subtitle: vec![
                Link {
                    text: "Playlist".into(),
                    target: None,
                },
                Link {
                    text: if playlist.owned {
                        "You".into()
                    } else {
                        "YouTube Music".into()
                    },
                    target: None,
                },
                Link {
                    text: "2026".into(),
                    target: None,
                },
            ],
            second_subtitle: Some(format!(
                "{} songs \u{2022} {}",
                playlist.tracks.len(),
                minutes(&playlist.tracks)
            )),
            description: None,
            art: art(&format!("playlist-{playlist_id}")),
            play: Some(PlaySource::Page {
                target: BrowseTarget::Playlist(playlist_id.into()),
                tracks: Vec::new(),
            }),
            editable: playlist.owned,
            privacy: playlist.owned.then_some(Privacy::Private),
            actions: Actions {
                save_ref: (!playlist.owned).then(|| playlist_id.to_owned()),
                saved: (!playlist.owned).then_some(true),
                ..Actions::default()
            },
        }),
        chips: Vec::new(),
        sections: vec![Section {
            title: None,
            strapline: None,
            layout: SectionLayout::List,
            items: first,
            more: None,
            continuation,
            ..Default::default()
        }],
        continuation: None,
    })
}

fn mood_page(id: &str) -> Page {
    let title = id.strip_prefix("mood-").unwrap_or(id);
    let catalog = catalog();
    Page {
        target: BrowseTarget::Mood(id.into()),
        header: Some(Header::Title {
            title: title.into(),
        }),
        chips: Vec::new(),
        sections: vec![
            section(
                "Featured playlists",
                SectionLayout::Carousel,
                catalog.playlists.iter().rev().map(playlist_item).collect(),
            ),
            section(
                "Albums",
                SectionLayout::Carousel,
                (0..8)
                    .map(|n| album_item(catalog, (n * 5 + title.len()) % 20))
                    .collect(),
            ),
        ],
        continuation: None,
    }
}

/// Every page the demo can show; `None` is a page it does not have.
pub fn page(target: &BrowseTarget) -> Option<Page> {
    match target {
        BrowseTarget::Home => Some(home(None)),
        BrowseTarget::HomeChip(id) => Some(home(id.strip_prefix("chip-"))),
        BrowseTarget::Explore | BrowseTarget::NewReleases | BrowseTarget::Charts => Some(explore()),
        BrowseTarget::MoodsAndGenres => Some(Page {
            target: target.clone(),
            header: Some(Header::Title {
                title: "Moods and genres".into(),
            }),
            chips: Vec::new(),
            sections: vec![section("Moods and genres", SectionLayout::Grid, moods())],
            continuation: None,
        }),
        BrowseTarget::Mood(id) => Some(mood_page(id)),
        BrowseTarget::Library(tab) => Some(library(*tab)),
        BrowseTarget::History => Some(Page {
            target: target.clone(),
            header: Some(Header::Title {
                title: "History".into(),
            }),
            chips: Vec::new(),
            sections: [("Today", 5, 6), ("Yesterday", 9, 4), ("This week", 13, 10)]
                .into_iter()
                .map(|(day, seed, count)| {
                    let rows = tracks_from(seed, count).into_iter().map(|mut track| {
                        track.actions.history_token = Some(format!("history-{day}-{}", track.key));
                        Item::Track(track)
                    });
                    section(day, SectionLayout::List, rows.collect())
                })
                .collect(),
            continuation: None,
        }),
        BrowseTarget::Album(id) => album_page(id),
        BrowseTarget::Artist(id) => artist_page(id),
        BrowseTarget::Page(id) if id.ends_with(SONGS_SHELF) => {
            let browse_id = id.strip_suffix(SONGS_SHELF)?;
            let mut page = artist_page(browse_id)?;
            let n = catalog()
                .artists
                .iter()
                .position(|artist| artist.browse_id == browse_id)?;
            let songs = catalog()
                .albums
                .iter()
                .filter(|album| album.artist == n)
                .flat_map(|album| album.tracks.iter().cloned())
                .map(Item::Track)
                .collect();
            page.header = Some(Header::Title {
                title: format!("{}: songs", catalog().artists[n].name),
            });
            page.sections = vec![section("Songs", SectionLayout::List, songs)];
            page.target = target.clone();
            Some(page)
        }
        BrowseTarget::Playlist(id) => playlist_page(id),
        _ => None,
    }
}

fn search(query: &str, filter: Option<SearchFilter>) -> SearchResults {
    let catalog = catalog();
    let needle = query.to_lowercase();
    let matches = |text: &str| needle.is_empty() || text.to_lowercase().contains(&needle);
    let mut songs: Vec<Item> = catalog
        .albums
        .iter()
        .flat_map(|album| album.tracks.iter())
        .filter(|track| matches(&track.title) || matches(&track.artists[0].text))
        .take(40)
        .cloned()
        .map(Item::Track)
        .collect();
    if songs.is_empty() {
        songs = tracks_from(query.len() as u64 + 3, 12)
            .into_iter()
            .map(Item::Track)
            .collect();
    }
    let albums: Vec<Item> = (0..catalog.albums.len())
        .filter(|n| {
            matches(&catalog.albums[*n].title)
                || matches(catalog.artists[catalog.albums[*n].artist].name)
        })
        .map(|n| album_item(catalog, n))
        .collect();
    let artists: Vec<Item> = (0..catalog.artists.len())
        .filter(|n| matches(catalog.artists[*n].name))
        .map(|n| artist_item(catalog, n))
        .collect();
    let playlists: Vec<Item> = catalog
        .playlists
        .iter()
        .filter(|playlist| matches(playlist.title))
        .map(playlist_item)
        .collect();
    let sections = match filter {
        Some(SearchFilter::Songs) | Some(SearchFilter::Videos) => {
            vec![section("Songs", SectionLayout::List, songs)]
        }
        Some(SearchFilter::Albums) => vec![section(
            "Albums",
            SectionLayout::Grid,
            if albums.is_empty() {
                (0..8).map(|n| album_item(catalog, n)).collect()
            } else {
                albums
            },
        )],
        Some(SearchFilter::Artists) => vec![section(
            "Artists",
            SectionLayout::List,
            if artists.is_empty() {
                (0..8).map(|n| artist_item(catalog, n)).collect()
            } else {
                artists
            },
        )],
        Some(_) => vec![section(
            "Playlists",
            SectionLayout::List,
            catalog.playlists.iter().map(playlist_item).collect(),
        )],
        None => {
            let top = artists
                .first()
                .cloned()
                .or_else(|| albums.first().cloned())
                .unwrap_or_else(|| songs[0].clone());
            let mut sections = vec![
                section("Top result", SectionLayout::Hero, vec![top]),
                section(
                    "Songs",
                    SectionLayout::List,
                    songs.into_iter().take(5).collect(),
                ),
            ];
            if !albums.is_empty() {
                sections.push(section("Albums", SectionLayout::Carousel, albums));
            }
            if !artists.is_empty() {
                sections.push(section("Artists", SectionLayout::Carousel, artists));
            }
            sections.push(section(
                "Community playlists",
                SectionLayout::Carousel,
                if playlists.is_empty() {
                    catalog.playlists.iter().map(playlist_item).collect()
                } else {
                    playlists
                },
            ));
            for section in &mut sections {
                section.filter = match section.title.as_deref() {
                    Some("Songs") => Some(SearchFilter::Songs),
                    Some("Albums") => Some(SearchFilter::Albums),
                    Some("Artists") => Some(SearchFilter::Artists),
                    Some("Community playlists") => Some(SearchFilter::CommunityPlaylists),
                    _ => None,
                };
            }
            sections
        }
    };
    SearchResults {
        query: query.into(),
        filter,
        correction: None,
        sections,
        continuation: None,
    }
}

fn suggestions(query: &str) -> Vec<Suggestion> {
    let catalog = catalog();
    let needle = query.to_lowercase();
    let mut out: Vec<Suggestion> = SONGS
        .iter()
        .chain(ALBUMS.iter())
        .filter(|text| {
            text.to_lowercase().starts_with(&needle)
                || text.to_lowercase().contains(&format!(" {needle}"))
        })
        .take(5)
        .map(|text| Suggestion::Query {
            text: text.to_lowercase(),
            from_history: false,
        })
        .collect();
    if out.is_empty() {
        out.push(Suggestion::Query {
            text: format!("{query} remix"),
            from_history: false,
        });
        out.push(Suggestion::Query {
            text: format!("{query} live"),
            from_history: true,
        });
    }
    if let Some(n) = (0..catalog.artists.len())
        .find(|n| catalog.artists[*n].name.to_lowercase().contains(&needle))
    {
        out.push(Suggestion::Item(artist_item(catalog, n)));
    }
    if let Some(n) = (0..catalog.albums.len())
        .find(|n| catalog.albums[*n].title.to_lowercase().contains(&needle))
    {
        out.push(Suggestion::Item(album_item(catalog, n)));
    }
    out
}

/// Backing vocals, one after every fourth line.
const ECHOES: [&str; 4] = [
    "(light it up)",
    "(oh, oh)",
    "(keep moving now)",
    "(we were younger)",
];

/// Whether the demo sings `key` as a duet, every other couplet from the
/// second voice on the other side of the lane.
pub fn duet(key: &str) -> bool {
    key.bytes().last().is_some_and(|byte| byte % 2 == 0)
}

/// Word synced the way Apple Music's are: syllable chunks on the longer
/// words, a backing vocal after every fourth line and an instrumental break
/// in place of every ninth.
fn lyrics(key: &str, duration_ms: u64) -> Lyrics {
    let offset = key.len() % LYRICS.len();
    let duet = duet(key);
    let mut lines = Vec::new();
    let mut at = 8_000;
    let mut n = 0;
    while at + 6_000 < duration_ms {
        if n % 9 == 8 {
            at += 7_000;
            n += 1;
            continue;
        }
        let text = LYRICS[(offset + n) % LYRICS.len()];
        let length = 3_600 + (n as u64 % 3) * 600;
        let opposite = duet && (n / 2) % 2 == 1;
        let agent = duet.then(|| if opposite { "v2" } else { "v1" }.to_owned());
        lines.push(LyricLine {
            start_ms: at,
            end_ms: Some(at + length),
            text: text.to_owned(),
            words: timed_words(text, at, length - 400),
            background: false,
            agent: agent.clone(),
            opposite_turn: opposite,
        });
        if n % 4 == 3 {
            let echo = ECHOES[(n / 4) % ECHOES.len()];
            let start = at + length / 2;
            let span = length / 2 + 900;
            lines.push(LyricLine {
                start_ms: start,
                end_ms: Some(start + span),
                text: echo.to_owned(),
                words: timed_words(echo, start, span - 200),
                background: true,
                agent,
                opposite_turn: opposite,
            });
        }
        at += length + 400;
        n += 1;
    }
    Lyrics {
        source: Some("Apple Music".into()),
        lines,
        synced: true,
        word_synced: true,
    }
}

/// `text` spread over `span_ms` by character count, words over six
/// characters split in two syllables.
fn timed_words(text: &str, start_ms: u64, span_ms: u64) -> Vec<LyricWord> {
    let chunks: Vec<(&str, bool)> = text
        .split_whitespace()
        .flat_map(|word| {
            let chars = word.chars().count();
            if chars > 6 {
                let (split, _) = word.char_indices().nth(chars / 2).unwrap_or((0, ' '));
                vec![(&word[..split], true), (&word[split..], false)]
            } else {
                vec![(word, false)]
            }
        })
        .collect();
    let total: usize = chunks.iter().map(|(chunk, _)| chunk.chars().count()).sum();
    let mut before = 0;
    chunks
        .into_iter()
        .map(|(chunk, joins_next)| {
            let chars = chunk.chars().count();
            let at = start_ms + span_ms * before as u64 / total.max(1) as u64;
            before += chars;
            LyricWord {
                start_ms: at,
                end_ms: start_ms + span_ms * before as u64 / total.max(1) as u64,
                text: chunk.to_owned(),
                joins_next,
            }
        })
        .collect()
}

fn related() -> Page {
    let catalog = catalog();
    Page {
        target: BrowseTarget::Page("related".into()),
        header: None,
        chips: Vec::new(),
        sections: vec![
            section(
                "You might also like",
                SectionLayout::TrackGrid,
                tracks_from(77, 8).into_iter().map(Item::Track).collect(),
            ),
            section(
                "Recommended playlists",
                SectionLayout::Carousel,
                catalog.playlists.iter().map(playlist_item).collect(),
            ),
            section(
                "Similar artists",
                SectionLayout::Carousel,
                (0..catalog.artists.len())
                    .map(|n| artist_item(catalog, n))
                    .collect(),
            ),
        ],
        continuation: None,
    }
}

fn tracks_for(source: &PlaySource) -> Vec<Track> {
    let catalog = catalog();
    let list = |page: Option<Page>| -> Option<Vec<Track>> {
        let tracks: Vec<Track> = page?
            .sections
            .into_iter()
            .find(|section| section.layout == SectionLayout::List)?
            .items
            .into_iter()
            .filter_map(|item| match item {
                Item::Track(track) => Some(track),
                _ => None,
            })
            .collect();
        (!tracks.is_empty()).then_some(tracks)
    };
    match source {
        PlaySource::Tracks { tracks } => tracks.clone(),
        PlaySource::Page {
            target: BrowseTarget::Playlist(id),
            ..
        } => catalog
            .playlists
            .iter()
            .find(|playlist| playlist.playlist_id == *id)
            .map(|playlist| playlist.tracks.clone())
            .unwrap_or_else(|| tracks_from(id.len() as u64, 25)),
        PlaySource::Page { target, .. } => list(page(target)).unwrap_or_else(|| tracks_from(7, 25)),
        PlaySource::Artist { key } => {
            list(page(&BrowseTarget::Page(format!("{key}{SONGS_SHELF}"))))
                .unwrap_or_else(|| tracks_from(key.len() as u64, 25))
        }
        PlaySource::PlaylistRadio { id } => tracks_from(id.len() as u64 * 7, 24),
        PlaySource::Radio { key } => {
            let seed = catalog
                .albums
                .iter()
                .flat_map(|album| album.tracks.iter())
                .find(|track| track.key == *key)
                .cloned();
            let mut tracks = tracks_from(key.len() as u64 * 13, 24);
            if let Some(seed) = seed {
                tracks.insert(0, seed);
            }
            tracks
        }
    }
}

struct DemoState {
    player: PlayerState,
    queue: QueueState,
    /// While playing: when `base_ms` was the position.
    since: Option<Instant>,
    base_ms: u64,
    session: SessionInfo,
    scrobbling: ScrobbleStatus,
}

impl DemoState {
    fn position(&self) -> u64 {
        let elapsed = self
            .since
            .map_or(0, |since| since.elapsed().as_millis() as u64);
        (self.base_ms + elapsed).min(self.player.duration_ms.unwrap_or(u64::MAX))
    }

    fn set_status(&mut self, status: Status) {
        self.base_ms = self.position();
        self.since = (status == Status::Playing).then(Instant::now);
        self.player.status = status;
        self.player.position_ms = self.base_ms;
    }

    fn load(&mut self, index: usize, play: bool) {
        let Some(track) = self.queue.tracks.get(index).cloned() else {
            self.player.track = None;
            self.queue.current = None;
            self.set_status(Status::Stopped);
            return;
        };
        self.queue.current = Some(index);
        self.player.duration_ms = track.duration_ms;
        self.player.stream = Some("opus 256 kbps".into());
        self.player.track = Some(track);
        self.base_ms = 0;
        self.since = None;
        self.player.position_ms = 0;
        self.set_status(if play {
            Status::Playing
        } else {
            Status::Paused
        });
    }

    fn snapshot(&mut self) -> PlayerState {
        self.player.position_ms = self.position();
        self.player.clone()
    }
}

fn account(name: &str, handle: &str, page_id: Option<&str>, selected: bool) -> Account {
    Account {
        name: name.into(),
        handle: Some(handle.into()),
        art: art(&format!("account-{name}")),
        page_id: page_id.map(Into::into),
        selected,
    }
}

fn session(signed_in: bool, page_id: Option<String>) -> SessionInfo {
    let account = signed_in.then(|| match page_id.as_deref() {
        Some(_) => account("Rivera Records", "@riverarecords", Some("demo-brand"), true),
        None => account("Alex Rivera", "@alexrivera", None, true),
    });
    SessionInfo {
        signed_in,
        account,
        premium: true,
    }
}

struct DemoShared {
    state: Mutex<DemoState>,
    events: Mutex<Option<mpsc::UnboundedSender<Event>>>,
    wake: Notify,
    ticker: Mutex<Option<AbortHandle>>,
}

impl DemoShared {
    fn emit(&self, event: Event) {
        if let Some(events) = self.events.lock().as_ref() {
            let _ = events.send(event);
        }
    }

    fn emit_player(&self) {
        let player = self.state.lock().snapshot();
        self.emit(Event::Player(player));
        self.wake.notify_one();
    }

    fn emit_queue(&self) {
        let queue = self.state.lock().queue.clone();
        self.emit(Event::Queue(queue));
    }

    fn signed_in(&self, session: SessionInfo) -> SessionInfo {
        self.state.lock().session = session.clone();
        session
    }

    fn scrobbling(&self, change: impl FnOnce(&mut ScrobbleStatus)) -> ScrobbleStatus {
        let mut state = self.state.lock();
        change(&mut state.scrobbling);
        state.scrobbling.clone()
    }
}

pub struct DemoBackend {
    shared: Arc<DemoShared>,
}

impl DemoBackend {
    pub fn new() -> Self {
        let signed_in = std::env::var("FORMALMUSIC_DEMO_SIGNED_OUT").ok().as_deref() != Some("1");
        let album = &catalog().albums[2];
        let mut state = DemoState {
            player: PlayerState {
                volume: 0.8,
                ..PlayerState::default()
            },
            queue: QueueState {
                tracks: album.tracks.clone(),
                current: None,
            },
            since: None,
            base_ms: 0,
            session: session(signed_in, None),
            scrobbling: ScrobbleStatus {
                lastfm: ScrobbleAccount {
                    connected: true,
                    connecting: false,
                },
                ..ScrobbleStatus::default()
            },
        };
        state.load(1, false);
        state.base_ms = 63_000;
        state.player.position_ms = 63_000;
        Self {
            shared: Arc::new(DemoShared {
                state: Mutex::new(state),
                events: Mutex::new(None),
                wake: Notify::new(),
                ticker: Mutex::new(None),
            }),
        }
    }
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Every demo album shares one animated cover: `demo/animated-cover.mp4` in
/// the cache dir, rendered by ffmpeg the first time it is asked for. Copy a
/// real Apple Music cover over it to measure with real footage.
pub async fn animated_cover() -> Option<PathBuf> {
    static COVER: tokio::sync::OnceCell<Option<PathBuf>> = tokio::sync::OnceCell::const_new();
    COVER
        .get_or_init(|| async {
            let path = crate::paths::cache_dir()
                .join("demo")
                .join("animated-cover.mp4");
            if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                tokio::fs::create_dir_all(path.parent()?).await.ok()?;
                let partial = path.with_extension("part.mp4");
                let status = crate::process::async_command("ffmpeg")
                    .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i"])
                    .arg("gradients=s=768x768:r=24:d=12:speed=0.02:n=4,noise=alls=10:allf=t")
                    .args(["-c:v", "libx264", "-b:v", "2M", "-pix_fmt", "yuv420p"])
                    .arg(&partial)
                    .status()
                    .await
                    .ok()?;
                if !status.success() {
                    return None;
                }
                tokio::fs::rename(&partial, &path).await.ok()?;
            }
            Some(path)
        })
        .await
        .clone()
}

/// Sends a position four times a second while playing and moves on at the
/// end of a track. Parked on `wake` while paused, so a paused demo costs nothing.
async fn tick(shared: Arc<DemoShared>) {
    loop {
        let playing = shared.state.lock().player.status == Status::Playing;
        if !playing {
            shared.wake.notified().await;
            continue;
        }
        tokio::time::sleep(TICK).await;
        let (position, ended) = {
            let state = shared.state.lock();
            if state.player.status != Status::Playing {
                continue;
            }
            let position = state.position();
            (
                position,
                state
                    .player
                    .duration_ms
                    .is_some_and(|duration| position >= duration),
            )
        };
        if ended {
            {
                let mut state = shared.state.lock();
                let current = state.queue.current.unwrap_or(0);
                let next = match state.player.repeat {
                    Repeat::One => Some(current),
                    Repeat::All => Some((current + 1) % state.queue.tracks.len().max(1)),
                    Repeat::Off => (current + 1 < state.queue.tracks.len()).then_some(current + 1),
                };
                match next {
                    Some(next) => state.load(next, true),
                    None => state.set_status(Status::Paused),
                }
            }
            shared.emit_player();
            shared.emit_queue();
        } else {
            let duration = shared.state.lock().player.duration_ms.unwrap_or(0);
            shared.emit(Event::Position {
                position_ms: position,
                buffered_ms: (position + 40_000).min(duration),
            });
        }
    }
}

fn demo_browsers() -> Vec<Browser> {
    [("helium", "Helium"), ("firefox", "Firefox")]
        .map(|(id, name)| Browser {
            id: id.into(),
            name: name.into(),
        })
        .into()
}

#[async_trait]
impl Backend for DemoBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Demo
    }

    fn start(&self, events: mpsc::UnboundedSender<Event>) {
        let _ = events.send(Event::Connection {
            status: ConnectionStatus::Online,
            error: None,
        });
        *self.shared.events.lock() = Some(events);
        self.shared.emit_player();
        self.shared.emit_queue();
        let task = tokio::spawn(tick(self.shared.clone()));
        *self.shared.ticker.lock() = Some(task.abort_handle());
    }

    fn stop(&self) {
        if let Some(task) = self.shared.ticker.lock().take() {
            task.abort();
        }
        *self.shared.events.lock() = None;
    }

    async fn session(&self) -> Result<SessionInfo> {
        Ok(self.shared.state.lock().session.clone())
    }

    async fn browse(&self, target: &BrowseTarget) -> Result<Page> {
        if !self.shared.state.lock().session.signed_in
            && matches!(target, BrowseTarget::Library(_) | BrowseTarget::History)
        {
            return Err(ClientError::SignedOut);
        }
        page(target).ok_or_else(|| ClientError::NotFound(format!("{target:?}")))
    }

    async fn more(
        &self,
        target: &BrowseTarget,
        section: Option<usize>,
        token: &Continuation,
    ) -> Result<ContinuationPage> {
        let token = &token.0;
        if section.is_none() {
            let n = token
                .strip_prefix("home-")
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| ClientError::NotFound(token.clone()))?;
            return Ok(home_more(n));
        }
        let BrowseTarget::Playlist(id) = target else {
            return Err(ClientError::NotFound(token.clone()));
        };
        let offset: usize = token
            .rsplit_once(':')
            .and_then(|(_, offset)| offset.parse().ok())
            .ok_or_else(|| ClientError::BadRequest(token.clone()))?;
        let playlist = catalog()
            .playlists
            .iter()
            .find(|playlist| playlist.playlist_id == *id)
            .ok_or_else(|| ClientError::NotFound(id.clone()))?;
        let items = playlist
            .tracks
            .iter()
            .skip(offset)
            .take(PLAYLIST_PAGE)
            .cloned()
            .map(Item::Track)
            .collect();
        let next = offset + PLAYLIST_PAGE;
        let continuation =
            (next < playlist.tracks.len()).then(|| Continuation(format!("playlist:{id}:{next}")));
        Ok(ContinuationPage {
            sections: Vec::new(),
            items,
            continuation,
        })
    }

    async fn search(&self, query: &str, filter: Option<SearchFilter>) -> Result<SearchResults> {
        Ok(search(query, filter))
    }

    async fn search_more(
        &self,
        _query: &str,
        _filter: Option<SearchFilter>,
        token: &Continuation,
    ) -> Result<ContinuationPage> {
        Err(ClientError::NotFound(token.0.clone()))
    }

    async fn suggestions(&self, query: &str) -> Result<Vec<Suggestion>> {
        Ok(suggestions(query))
    }

    fn search_filters(&self) -> Vec<SearchFilter> {
        let signed_in = self.shared.state.lock().session.signed_in;
        SearchFilter::ALL
            .into_iter()
            .filter(|filter| signed_in || *filter != SearchFilter::Library)
            .collect()
    }

    async fn lyrics(&self, key: &str) -> Result<Option<Lyrics>> {
        let duration = catalog()
            .albums
            .iter()
            .flat_map(|album| album.tracks.iter())
            .find(|track| track.key == key)
            .and_then(|track| track.duration_ms)
            .unwrap_or(200_000);
        Ok(Some(lyrics(key, duration)))
    }

    async fn related(&self, _key: &str) -> Result<Page> {
        Ok(related())
    }

    async fn artwork(&self, art: &Art, _hq: bool) -> Result<Vec<u8>> {
        Err(ClientError::NotFound(art.id.clone()))
    }

    async fn share_url(&self, item: &Item) -> Result<Option<String>> {
        const BASE: &str = "https://music.youtube.com";
        Ok(match item {
            Item::Track(track) => Some(format!("{BASE}/watch?v={}", track.key)),
            Item::Album { browse_id, .. }
            | Item::Artist { browse_id, .. }
            | Item::Podcast { browse_id, .. } => Some(format!("{BASE}/browse/{browse_id}")),
            _ => None,
        })
    }

    async fn play(&self, source: PlaySource, start_index: usize, shuffle: bool) -> Result<()> {
        let mut tracks = tracks_for(&source);
        if shuffle {
            let mut rng = Rng(tracks.len() as u64 * 2654435761 | 1);
            for n in (1..tracks.len()).rev() {
                tracks.swap(n, rng.below(n as u64 + 1) as usize);
            }
        }
        {
            let mut state = self.shared.state.lock();
            state.queue = QueueState {
                tracks,
                current: None,
            };
            state.player.shuffle = shuffle;
            state.load(if shuffle { 0 } else { start_index }, true);
        }
        self.shared.emit_player();
        self.shared.emit_queue();
        Ok(())
    }

    async fn enqueue(&self, tracks: Vec<Track>, position: EnqueuePosition) -> Result<()> {
        let shared = &self.shared;
        let start = {
            let mut state = shared.state.lock();
            let at = match position {
                EnqueuePosition::Next => state.queue.current.map_or(0, |current| current + 1),
                EnqueuePosition::End => state.queue.tracks.len(),
            };
            let empty = state.queue.tracks.is_empty();
            for (offset, track) in tracks.into_iter().enumerate() {
                state.queue.tracks.insert(at + offset, track);
            }
            empty
        };
        if start {
            shared.state.lock().load(0, true);
            shared.emit_player();
        }
        shared.emit_queue();
        Ok(())
    }

    async fn control(&self, control: Control) -> Result<()> {
        let shared = &self.shared;
        match control {
            Control::Remove(index) => {
                {
                    let mut state = shared.state.lock();
                    if index < state.queue.tracks.len() {
                        state.queue.tracks.remove(index);
                        match state.queue.current {
                            Some(current) if current > index => {
                                state.queue.current = Some(current - 1)
                            }
                            Some(current) if current == index => {
                                let last = state.queue.tracks.len().saturating_sub(1);
                                state.load(index.min(last), true);
                            }
                            _ => {}
                        }
                    }
                }
                shared.emit_queue();
            }
            Control::Move { from, to } => {
                {
                    let mut state = shared.state.lock();
                    if from < state.queue.tracks.len() && to < state.queue.tracks.len() {
                        let track = state.queue.tracks.remove(from);
                        state.queue.tracks.insert(to, track);
                        if let Some(current) = state.queue.current {
                            state.queue.current = Some(if current == from {
                                to
                            } else if from < current && current <= to {
                                current - 1
                            } else if to <= current && current < from {
                                current + 1
                            } else {
                                current
                            });
                        }
                    }
                }
                shared.emit_queue();
            }
            Control::Clear => {
                {
                    let mut state = shared.state.lock();
                    let current = state
                        .queue
                        .current
                        .and_then(|current| state.queue.tracks.get(current).cloned());
                    state.queue.tracks = current.into_iter().collect();
                    state.queue.current = (!state.queue.tracks.is_empty()).then_some(0);
                }
                shared.emit_queue();
            }
            Control::Jump(index) => {
                shared.state.lock().load(index, true);
                shared.emit_player();
                shared.emit_queue();
            }
            Control::Toggle | Control::Pause => {
                {
                    let mut state = shared.state.lock();
                    let next = match control {
                        Control::Pause => Status::Paused,
                        _ if state.player.status == Status::Playing => Status::Paused,
                        _ => Status::Playing,
                    };
                    if state.player.track.is_some() {
                        state.set_status(next);
                    }
                }
                shared.emit_player();
            }
            Control::Next | Control::Previous => {
                {
                    let mut state = shared.state.lock();
                    let current = state.queue.current.unwrap_or(0);
                    let len = state.queue.tracks.len();
                    let target = if control == Control::Next {
                        (current + 1).min(len.saturating_sub(1))
                    } else if state.position() > 3_000 {
                        current
                    } else {
                        current.saturating_sub(1)
                    };
                    state.load(target, true);
                }
                shared.emit_player();
                shared.emit_queue();
            }
            Control::Seek(position_ms) => {
                {
                    let mut state = shared.state.lock();
                    state.base_ms = position_ms;
                    if state.since.is_some() {
                        state.since = Some(Instant::now());
                    }
                    state.player.position_ms = position_ms;
                }
                let duration = shared.state.lock().player.duration_ms.unwrap_or(0);
                shared.emit(Event::Position {
                    position_ms,
                    buffered_ms: (position_ms + 40_000).min(duration),
                });
            }
            Control::Volume(volume) => {
                shared.state.lock().player.volume = volume;
                shared.emit_player();
            }
            Control::Muted(muted) => {
                shared.state.lock().player.muted = muted;
                shared.emit_player();
            }
            Control::Repeat(repeat) => {
                shared.state.lock().player.repeat = repeat;
                shared.emit_player();
            }
            Control::Shuffle(shuffle) => {
                shared.state.lock().player.shuffle = shuffle;
                shared.emit_player();
            }
        }
        Ok(())
    }

    async fn rate(&self, _rate_ref: &str, _rating: Rating) -> Result<()> {
        self.shared.emit(Event::LibraryChanged {
            scope: LibraryScope::Likes,
        });
        Ok(())
    }

    async fn follow(&self, _follow_ref: &str, _follow: bool) -> Result<()> {
        Ok(())
    }

    async fn save(&self, _save_ref: &str, _saved: bool) -> Result<()> {
        self.shared.emit(Event::LibraryChanged {
            scope: LibraryScope::Songs,
        });
        Ok(())
    }

    async fn remove_from_history(&self, _token: &str) -> Result<()> {
        Ok(())
    }

    async fn create_playlist(&self, _title: String, _keys: Vec<String>) -> Result<String> {
        Ok("PLdemo-new".into())
    }

    async fn add_to_playlist(&self, _playlist_id: &str, _keys: Vec<String>) -> Result<()> {
        Ok(())
    }

    async fn remove_from_playlist(&self, _playlist_id: &str, _index: usize) -> Result<()> {
        Ok(())
    }

    async fn move_in_playlist(&self, _playlist_id: &str, _from: usize, _to: usize) -> Result<()> {
        Ok(())
    }

    fn playlists_reorder(&self) -> bool {
        true
    }

    async fn edit_playlist(&self, _playlist_id: &str, _details: PlaylistDetails) -> Result<()> {
        Ok(())
    }

    async fn delete_playlist(&self, _playlist_id: &str) -> Result<()> {
        Ok(())
    }

    async fn sign_in(&self, cookies: String) -> Result<SessionInfo> {
        if !cookies.contains('=') {
            return Err(ClientError::BadRequest(
                "That does not look like a Cookie header. It should contain name=value pairs."
                    .into(),
            ));
        }
        Ok(self.shared.signed_in(session(true, None)))
    }

    async fn browsers(&self) -> Result<Browsers> {
        Ok(Browsers {
            installed: demo_browsers(),
            default: Some("helium".into()),
        })
    }

    async fn browser_sign_in(&self, _browser: Option<String>) -> Result<SessionInfo> {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Ok(self.shared.signed_in(session(true, None)))
    }

    async fn browser_profiles(&self) -> Result<Vec<ProfileBrowser>> {
        Ok(vec![ProfileBrowser {
            browser: demo_browsers().remove(0),
            profiles: vec![
                BrowserProfile {
                    path: "Default".into(),
                    name: "Personal".into(),
                    email: Some("demo@example.com".into()),
                },
                BrowserProfile {
                    path: "Profile 1".into(),
                    name: "Work".into(),
                    email: None,
                },
            ],
        }])
    }

    async fn import_profile(&self, _profile: &str) -> Result<SessionInfo> {
        Ok(self.shared.signed_in(session(true, None)))
    }

    async fn sign_out(&self) -> Result<()> {
        let session = self.shared.signed_in(session(false, None));
        self.shared.emit(Event::Session(session));
        Ok(())
    }

    async fn accounts(&self) -> Result<Vec<Account>> {
        let brand = self
            .shared
            .state
            .lock()
            .session
            .account
            .as_ref()
            .and_then(|account| account.page_id.clone())
            .is_some();
        Ok(vec![
            account("Alex Rivera", "@alexrivera", None, !brand),
            account(
                "Rivera Records",
                "@riverarecords",
                Some("demo-brand"),
                brand,
            ),
        ])
    }

    async fn switch_account(&self, page_id: Option<String>) -> Result<SessionInfo> {
        Ok(self.shared.signed_in(session(true, page_id)))
    }

    async fn scrobbling(&self) -> Result<ScrobbleStatus> {
        Ok(self.shared.state.lock().scrobbling.clone())
    }

    async fn connect_lastfm(&self, _app: Option<LastFmApp>) -> Result<ScrobbleStatus> {
        Ok(self
            .shared
            .scrobbling(|status| status.lastfm.connected = true))
    }

    async fn connect_listenbrainz(&self, _token: String) -> Result<ScrobbleStatus> {
        Ok(self
            .shared
            .scrobbling(|status| status.listenbrainz.connected = true))
    }

    async fn disconnect_scrobbler(&self, service: ScrobbleService) -> Result<ScrobbleStatus> {
        Ok(self.shared.scrobbling(|status| match service {
            ScrobbleService::LastFm => status.lastfm.connected = false,
            ScrobbleService::ListenBrainz => status.listenbrainz.connected = false,
        }))
    }

    async fn apply_settings(&self, _settings: &Settings) -> Result<()> {
        Ok(())
    }

    async fn preview_equalizer(&self, _equalizer: crate::equalizer::Equalizer) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_link_on_home_opens_a_page() {
        let home = page(&BrowseTarget::Home).unwrap();
        for section in &home.sections {
            for item in &section.items {
                let target = match item {
                    Item::Album { browse_id, .. } => BrowseTarget::Album(browse_id.clone()),
                    Item::Artist { browse_id, .. } => BrowseTarget::Artist(browse_id.clone()),
                    Item::Playlist { playlist_id, .. } => {
                        BrowseTarget::Playlist(playlist_id.clone())
                    }
                    _ => continue,
                };
                assert!(page(&target).is_some(), "{target:?}");
            }
        }
    }

    #[tokio::test]
    async fn the_long_playlist_pages_through_every_track() {
        let demo = DemoBackend::new();
        let target = BrowseTarget::Playlist(LONG_PLAYLIST.into());
        let page = demo.browse(&target).await.unwrap();
        let mut count = page.sections[0].items.len();
        let mut token = page.sections[0].continuation.clone();
        while let Some(next) = token {
            let more = demo.more(&target, Some(0), &next).await.unwrap();
            count += more.items.len();
            token = more.continuation;
        }
        assert_eq!(count, 1000);
    }
}
