# Parity with music.youtube.com

Checklist of the web app's user-facing features and routes against FormalMusic.
"yes" names the file that implements it, "partial" says what is missing, "no"
means nothing in the repo does it. Paths are relative to `crates/`. A feature
the daemon supports but no screen reaches is "partial", since the user cannot
use it.

Sources: the web app's own shortcut overlay (read from music.youtube.com with
`?`, 2026-10-06), its left nav and home chips, the
[ytmusicapi reference](https://ytmusicapi.readthedocs.io/en/latest/reference/index.html)
function list (browsing, explore, watch, library, playlists, podcasts,
uploads), and YouTube Music's
[audio quality help page](https://support.google.com/youtubemusic/answer/9076559).
The web context menu and settings page could not be opened headless, so those
rows come from ytmusicapi and the help page, not from the live menus.

## Routes and pages

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Home with shelves | `/` | yes | `desktop/src/page.rs`, `desktop/src/shelves.rs`, backend `innertube/src/parse/page.rs` |
| Home mood chips | Relax, Sleep, Focus, ... | yes | `desktop/src/page.rs` (`chip_target`) |
| Home infinite scroll | continuation | yes | `core/src/store.rs` (`load_more`) |
| Explore | `/explore` | yes | `desktop/src/sidebar.rs` |
| New releases | `/new_releases` | yes | Explore's button, `innertube/src/parse/items/navigation_button.rs` (`Item::Shortcut`), `desktop/src/shelves.rs` (`shortcut_tile`) |
| Charts | `/charts` | yes | Same as new releases |
| Moods and genres landing | `/moods_and_genres` | yes | Same as new releases; mood tiles open from `desktop/src/shelves.rs` (`mood_tile`) |
| Mood or genre category | `/moods_and_genres_category` | yes | `desktop/src/actions.rs` (`target_of`) |
| Library, Playlists tab | `/library` | yes | `desktop/src/page.rs` (`chip_target`) |
| Library, Songs, Albums, Artists, Subscriptions, Podcasts tabs | `/library/*` | yes | `desktop/src/page.rs` |
| Library, Uploads tab | `/library/uploads` | partial | Lists uploads (`innertube/src/parse/mod.rs`); no upload or delete |
| Liked music | `LM` playlist | yes | `desktop/src/sidebar.rs` playlist list, `desktop/src/page.rs` |
| History | `/history` | yes | Sidebar entry in `desktop/src/sidebar.rs`; day shelves (Today, Yesterday, ...) come from the response, `innertube/src/parse/shelves.rs` |
| Episodes for Later | `SE` playlist in sidebar | yes | Sidebar row when signed in, `app/src/root.rs` (`sidebar_nodes`); browses as a playlist |
| Search results | `/search?q=` | yes | `desktop/src/topbar.rs`, `desktop/src/page.rs` |
| Search suggestions and history | dropdown | yes | `desktop/src/topbar.rs` (`Suggestion::Query`, `from_history`) |
| Search filters | Songs, Videos, Albums, Featured playlists, Community playlists, Artists, Podcasts, Episodes, Profiles | yes | `desktop/src/page.rs` (`SEARCH_FILTERS`) |
| Search within library | library search | yes | Library chip in `desktop/src/page.rs`; `innertube/src/parse/search.rs` reads the selected tab |
| Delete a search history entry | x on suggestion | no | ytmusicapi `remove_search_suggestions`, not in `api` |
| Search top result card | Top result | yes | `desktop/src/shelves.rs` |
| Album page | `/browse/MPREb...` | yes | `desktop/src/header.rs`, `desktop/src/page.rs` |
| Artist page | `/channel/UC...` | yes | `desktop/src/header.rs` (`Header::Artist`) |
| Artist "see all" shelves | More | yes | `desktop/src/shelves.rs` (`ArtistShelf`) |
| Playlist page | `/playlist?list=` | yes | `desktop/src/header.rs`, `desktop/src/page.rs` |
| Podcast page | `/podcast/` | partial | Browses and lists episodes; no follow button of its own beyond `Save to library` |
| Episode page | `/episode/` | partial | `BrowseTarget::Episode` browses, plays as `TrackKind::Episode`; no resume position, no "save for later" |
| Channel and user pages | `get_user`, `get_channel` | partial | Reached as artist or podcast links and from the Profiles search filter; no page of their own |
| Watch page, Up next | `/watch` | yes | `desktop/src/now_playing.rs` (`QueueView`) |
| Watch page, Lyrics | tab | yes | `desktop/src/lyrics.rs`, `extras/src/lyrics/`; word-synced, beyond the web app |
| Watch page, Related | tab | yes | `desktop/src/now_playing.rs`, `core/src/store.rs` (`load_related`) |
| Song credits | `get_song_credits` | no | Kopuz has no credits call for YouTube Music |
| Taste profile (pick artists) | `get_tasteprofile` | no | Kopuz has no taste profile call |
| Account switcher | avatar menu | yes | `desktop/src/topbar.rs`, `daemon/src/session.rs` |
| Settings page | avatar menu | partial | `desktop/src/settings.rs`, see Settings |

## Playback and player

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Play, pause, next, previous, seek bar | player bar | yes | `desktop/src/player_bar.rs`, `daemon/src/playback.rs` |
| Volume and mute | slider | yes | `desktop/src/player_bar.rs` |
| Shuffle and repeat (off, all, one) | buttons | yes | `desktop/src/player_bar.rs`, `daemon/src/queue.rs` |
| Like and dislike | thumbs | yes | `desktop/src/header.rs` (`rating_buttons`), `daemon/src/daemon.rs` |
| Expanded player | full screen player | yes | `desktop/src/now_playing.rs` |
| Song or Video toggle | switch | yes | `desktop/src/music_video.rs`, `desktop/src/now_playing.rs` |
| Autoplay radio keeps queue filled | autoplay | yes | `daemon/src/queue.rs` (`RADIO_LOW_WATER`) |
| Queue reorder, remove | drag | yes | `desktop/src/now_playing.rs` (`QueueDrag`) |
| Clear queue | button | yes | Clear above Up next, `app/src/now_playing.rs`; Kopuz clears the songs after the playing one |
| Save queue as playlist | menu | yes | Save as playlist above Up next, `app/src/sheets.rs` (`save_queue`) |
| Gapless playback | automatic | yes | `player/src/engine.rs` |
| Loudness normalisation | "stable volume" | yes | `player/src/gain.rs`, `daemon/src/config.rs` |
| Media keys and now-playing widgets | OS media session | yes | `daemon/src/mpris.rs` (Linux only) |
| Play reporting for History and recommendations | automatic | yes | `daemon/src/tracking.rs` |

## Context menu and card actions

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Start radio | track menu | yes | `desktop/src/actions.rs` (`track_menu`) |
| Play next | track menu | yes | `desktop/src/actions.rs` |
| Add to queue | track menu | yes | `desktop/src/actions.rs` |
| Add to playlist | track menu | yes | `desktop/src/actions.rs` (`playlist_picker`) |
| Remove from playlist | playlist track menu | yes | `desktop/src/actions.rs` |
| Save song to library | track menu | yes | `desktop/src/actions.rs` (`track_menu`), `innertube/src/parse/items/mod.rs` (`library_toggle`), `Command::SetSongInLibrary`; rows from the `next` queue carry no toggle |
| Save album or playlist to library | menu and header | yes | `desktop/src/actions.rs`, `desktop/src/header.rs` |
| Like, dislike from menu | track menu | yes | `desktop/src/actions.rs` |
| Go to album, go to artist | track menu | yes | `desktop/src/actions.rs` |
| Remove from queue | queue menu | yes | `desktop/src/now_playing.rs` |
| Remove from history | history menu | yes | `desktop/src/actions.rs` (`track_menu`), `core/src/store.rs` (`remove_from_history`) |
| Share, copy link | menu | yes | `desktop/src/actions.rs` (`share_url`): every song, video, album, playlist, artist and podcast menu, the player bar's and the expanded player's, copies the web app's link with a "Link copied" toast |
| Report, not interested | menu | no | |
| Shuffle play an album or playlist | menu | yes | `desktop/src/actions.rs` |
| Subscribe to an artist | artist header | yes | `desktop/src/header.rs`, `daemon/src/daemon.rs` |
| Hover play button on cards | cards | yes | `desktop/src/shelves.rs`, `desktop/src/actions.rs` (`play_item`) |

## Playlists and library edits

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Create playlist | New playlist | partial | `desktop/src/new_playlist.rs` takes a title only; privacy and description exist in `Command::CreatePlaylist` |
| Rename, describe, change privacy | edit playlist | yes | "Edit playlist" in `desktop/src/header.rs` opens `desktop/src/edit_playlist.rs` |
| Reorder tracks in a playlist | drag | yes | `desktop/src/shelves.rs` (`RowDrag`), `core/src/store.rs` (`move_in_playlist`) |
| Delete playlist | menu | yes | `desktop/src/edit_playlist.rs`, after a confirmation |
| Add whole playlist into another | menu | partial | `PlaylistEdit::AddPlaylist`, no UI |
| Collaborative playlists | `join_collaborative_playlist` | no | Kopuz has no collaboration call |
| Upload songs | `upload_song` | no | Kopuz's `upload_track` is implemented for Clippsly only, not YouTube Music |
| Delete uploads | `delete_upload_entity` | no | Kopuz has no call for it |
| Subscribe to a podcast | podcast page | partial | Via `Save to library` where the header carries a playlist id |

## Keyboard shortcuts

Read from the web app's `?` overlay. FormalMusic's single keys and `g` chords
are in `desktop/src/shortcuts.rs`, the modifier bindings in `desktop/src/app.rs`
(`init`).

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Play or pause | `Space`, `;` | yes | `desktop/src/shortcuts.rs` |
| Next song | `j`, `Shift+n` | yes | `desktop/src/shortcuts.rs`; `n` too |
| Previous song | `k`, `Shift+p` | yes | `desktop/src/shortcuts.rs`; `p` too |
| Forward 10s | `l`, `Shift+Right` | yes | `desktop/src/shortcuts.rs` |
| Back 10s | `h`, `Shift+Left` | yes | `desktop/src/shortcuts.rs` |
| Forward or back 1s | `Shift+l`, `Shift+h` | yes | `desktop/src/shortcuts.rs` |
| Shuffle | `s` | yes | `desktop/src/shortcuts.rs` |
| Toggle repeat | `r` | yes | `desktop/src/shortcuts.rs` |
| Volume up, down | `=`, `-` | yes | `desktop/src/shortcuts.rs` (`Shift+Up` and `Shift+Down` too) |
| Mute | `m` | yes | `desktop/src/shortcuts.rs` |
| Toggle queue or expanded player | `q`, `Esc` | yes | `desktop/src/shortcuts.rs`, `desktop/src/app.rs` |
| Full screen | `f` | yes | `f` opens the expanded player; Window > Enter Full Screen (Ctrl+Cmd+F) on macOS, `app/src/root.rs` (`menus`) |
| Like current song | `+` | yes | `desktop/src/shortcuts.rs` |
| Dislike current song | `_` | yes | `desktop/src/shortcuts.rs` |
| Go to Home, Explore, Library, Settings | `gh`, `ge`, `gl`, `g,` | yes | `desktop/src/shortcuts.rs` (`resolve`), `Ctrl+,` for Settings too |
| Search | `/` | yes | `desktop/src/shortcuts.rs`, also `Ctrl+F` |
| Shortcuts overlay | `?` | yes | `desktop/src/shortcuts.rs` (`overlay`) |
| Back and forward in history | `Alt+Left`, `Alt+Right` | yes | `desktop/src/app.rs` |

## Settings

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Audio quality (Low, Normal, High, Always high) | playback settings | yes | Auto, Low, Normal, High in `desktop/src/settings.rs`, written to `config.json` and applied by `daemon/src/config.rs` (`Preferences`) through `Command::ReloadSettings` |
| Premium bitrate | automatic with Premium | yes | `daemon/src/streams.rs` |
| Autoplay | playback settings | yes | `desktop/src/settings.rs`; `daemon/src/playback.rs` (`autoplay`) turns a list into radio from its last track |
| Show or hide music videos (audio only) | playback settings | yes | Song/Video switch in `desktop/src/music_video.rs`; no persistent default |
| Restrict explicit content | playback settings | yes | "Skip explicit songs" in `desktop/src/settings.rs`; `daemon/src/playback.rs` keeps explicit tracks out of the queue |
| Pause watch history | privacy | yes | `desktop/src/settings.rs`, `daemon/src/config.rs` (`pauseHistory` over `reportHistory`) |
| Delete watch history | privacy | no | Kopuz removes single history entries only (`remove_from_history`) |
| Notifications | account settings | yes | "Notify when the song changes in the background" in Settings, `app/src/player.rs` (`announce`) |
| Connected apps, scrobbling | account settings | yes | Last.fm and ListenBrainz, `desktop/src/settings.rs`, `daemon/src/scrobble/` |
| Sign in, sign out, brand accounts | avatar menu | yes | `desktop/src/signin.rs`, `daemon/src/signin/` |

## Look on macOS

The macOS window follows Music on macOS 27, checked against Music.app captured on the macbook
and the macOS 26 marketing shot. Music's player is a Liquid Glass capsule floating over the
content: shuffle, previous, play, next and repeat leading, the song on two lines over a thin
progress line, then more, lyrics, queue and volume (nd `bottom_accessory` with
`TransportLayout::Pill`). Top-level pages carry a large title and the toolbar no window title.
macOS 27 changes no part of Music's player; system-wide, sidebars reach the window's edge and
keep their icons' colour. Not matched yet: Search as a sidebar row, the account row at the
sidebar's foot and Music's red symbol tint.

Sources:
[9to5Mac, macOS 27 changes](https://9to5mac.com/2026/06/09/macos-27-golden-gate-includes-these-changes-that-tahoe-critics-will-appreciate/),
[iPhoneIslam, Liquid Glass in iOS 27 and macOS 27](https://iphoneislam.com/language/en/2026/06/apple-responds-to-complaints-how-it-fixed-the-liquid-glass-interface-in-ios-27-and-mac/168187).

## Gaps

Most used first.

1. Clear queue and save queue as playlist. ~2 hours.
2. Episodes for Later entry, podcast resume position, episode save. ~a day.
3. Desktop notifications on track change. ~2 hours.
4. Song credits, taste profile, uploads, collaborative playlists. Each ~a day or more; uploads need new protocol work.
