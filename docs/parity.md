# Parity with music.youtube.com

Checklist of the web app's user-facing features and routes against FormalMusic.
"yes" names the file that implements it, "partial" says what is missing, "no"
means nothing does it. Paths are relative to `crates/`; what kopuzd does
happens in FormalSnake/kopuz at the rev `Cargo.toml` pins. A feature the
daemon supports but no screen reaches is "partial", since the user cannot
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
| Home with shelves | `/` | yes | `desktop/src/page.rs`, `desktop/src/shelves.rs`, pages from kopuzd through `core/src/convert.rs` (`page`) |
| Home mood chips | Relax, Sleep, Focus, ... | yes | `desktop/src/page.rs` (`chip_target`) |
| Home infinite scroll | continuation | yes | `core/src/store.rs` (`load_more`) |
| Explore | `/explore` | yes | `desktop/src/sidebar.rs` |
| New releases | `/new_releases` | yes | Explore's button, `core/src/convert.rs` (`Item::Shortcut`), `desktop/src/shelves.rs` (`shortcut_tile`) |
| Charts | `/charts` | yes | Same as new releases |
| Moods and genres landing | `/moods_and_genres` | yes | Same as new releases; mood tiles open from `desktop/src/shelves.rs` (`mood_tile`) |
| Mood or genre category | `/moods_and_genres_category` | yes | `desktop/src/actions.rs` (`target_of`) |
| Library, Playlists tab | `/library` | yes | `desktop/src/page.rs` (`chip_target`) |
| Library, Songs, Albums, Artists, Subscriptions, Podcasts tabs | `/library/*` | yes | `desktop/src/page.rs` |
| Library, Uploads tab | `/library/uploads` | partial | Lists uploads through kopuzd; no upload or delete |
| Liked music | `LM` playlist | yes | `desktop/src/sidebar.rs` playlist list, `desktop/src/page.rs` |
| History | `/history` | yes | Sidebar entry in `desktop/src/sidebar.rs`; day shelves (Today, Yesterday, ...) come from kopuzd |
| Episodes for Later | `SE` playlist in sidebar | no | No entry; the playlist would browse generically |
| Search results | `/search?q=` | yes | `desktop/src/topbar.rs`, `desktop/src/page.rs` |
| Search suggestions and history | dropdown | yes | `desktop/src/topbar.rs` (`Suggestion::Query`, `from_history`) |
| Search filters | Songs, Videos, Albums, Featured playlists, Community playlists, Artists, Podcasts, Episodes, Profiles | yes | `desktop/src/page.rs` (`SEARCH_FILTERS`) |
| Search within library | library search | yes | Library chip in `desktop/src/page.rs`, kopuzd's `library` filter |
| Delete a search history entry | x on suggestion | no | Not in kopuz's API |
| Search top result card | Top result | yes | `desktop/src/shelves.rs` |
| Album page | `/browse/MPREb...` | yes | `desktop/src/header.rs`, `desktop/src/page.rs` |
| Artist page | `/channel/UC...` | yes | `desktop/src/header.rs` (`Header::Artist`) |
| Artist "see all" shelves | More | yes | `desktop/src/shelves.rs`, the shelf's page in `core/src/convert.rs` (`section`) |
| Playlist page | `/playlist?list=` | yes | `desktop/src/header.rs`, `desktop/src/page.rs` |
| Podcast page | `/podcast/` | partial | Browses and lists episodes; no follow button of its own beyond `Save to library` |
| Episode page | `/episode/` | partial | `BrowseTarget::Episode` browses, plays as `TrackKind::Episode`; no resume position, no "save for later" |
| Channel and user pages | `get_user`, `get_channel` | partial | Reached as artist or podcast links and from the Profiles search filter; no page of their own |
| Watch page, Up next | `/watch` | yes | `desktop/src/now_playing.rs` (`QueueView`) |
| Watch page, Lyrics | tab | yes | `desktop/src/lyrics.rs`; kopuzd picks Apple Music, YouTube Music or LRCLIB, `core/src/convert.rs` (`lyrics`); word-synced, beyond the web app |
| Watch page, Related | tab | yes | `desktop/src/now_playing.rs`, `core/src/store.rs` (`load_related`) |
| Song credits | `get_song_credits` | no | |
| Taste profile (pick artists) | `get_tasteprofile` | no | |
| Account switcher | avatar menu | yes | `desktop/src/topbar.rs`, kopuz `SourceApi::accounts` and `switch_account` |
| Settings page | avatar menu | partial | `desktop/src/settings.rs`, see Settings |

## Playback and player

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Play, pause, next, previous, seek bar | player bar | yes | `desktop/src/player_bar.rs`, kopuzd |
| Volume and mute | slider | yes | `desktop/src/player_bar.rs` |
| Shuffle and repeat (off, all, one) | buttons | yes | `desktop/src/player_bar.rs`, kopuzd's queue |
| Like and dislike | thumbs | yes | `desktop/src/header.rs` (`rating_buttons`), kopuz `LibraryApi::rate` |
| Expanded player | full screen player | yes | `desktop/src/now_playing.rs` |
| Song or Video toggle | switch | no | kopuzd has no video stream and no song or video counterpart on a row |
| Autoplay radio keeps queue filled | autoplay | partial | A track radio tops itself up in kopuzd; a list that runs out does not turn into radio |
| Queue reorder, remove | drag | yes | `desktop/src/now_playing.rs` (`QueueDrag`) |
| Clear queue | button | partial | `MusicStore::clear_queue`, no control in `desktop/src` |
| Save queue as playlist | menu | no | |
| Gapless playback | automatic | yes | kopuzd's engine |
| Loudness normalisation | "stable volume" | yes | kopuzd, from `normalisation` in `config.json` |
| Media keys and now-playing widgets | OS media session | yes | kopuzd (MPRIS, SMTC) |
| Play reporting for History and recommendations | automatic | yes | kopuzd |

## Context menu and card actions

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Start radio | track menu | yes | `desktop/src/actions.rs` (`track_menu`) |
| Play next | track menu | yes | `desktop/src/actions.rs` |
| Add to queue | track menu | yes | `desktop/src/actions.rs` |
| Add to playlist | track menu | yes | `desktop/src/actions.rs` (`playlist_picker`) |
| Remove from playlist | playlist track menu | yes | `desktop/src/actions.rs` |
| Save song to library | track menu | yes | `desktop/src/actions.rs` (`track_menu`), the row's `Actions::save_ref`; queue rows carry no toggle |
| Save album or playlist to library | menu and header | yes | `desktop/src/actions.rs`, `desktop/src/header.rs` |
| Like, dislike from menu | track menu | yes | `desktop/src/actions.rs` |
| Go to album, go to artist | track menu | yes | `desktop/src/actions.rs` |
| Remove from queue | queue menu | yes | `desktop/src/now_playing.rs` |
| Remove from history | history menu | yes | `desktop/src/actions.rs` (`track_menu`), `core/src/store.rs` (`remove_from_history`) |
| Share, copy link | menu | partial | `desktop/src/actions.rs` (`share_row`): song, video, album, artist and podcast menus copy the link kopuzd gives; playlists have none until kopuz has a web URL for them |
| Report, not interested | menu | no | |
| Shuffle play an album or playlist | menu | yes | `desktop/src/actions.rs` |
| Subscribe to an artist | artist header | yes | `desktop/src/header.rs`, kopuz `LibraryApi::follow` |
| Hover play button on cards | cards | yes | `desktop/src/shelves.rs`, `desktop/src/actions.rs` (`play_item`) |

## Playlists and library edits

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Create playlist | New playlist | partial | `desktop/src/new_playlist.rs` takes a title only, as kopuz's `create_playlist` does |
| Rename, describe, change privacy | edit playlist | yes | "Edit playlist" in `desktop/src/header.rs` opens `desktop/src/edit_playlist.rs` |
| Reorder tracks in a playlist | drag | no | kopuzd's YouTube Music source takes no reorder (`PlaylistCapability::AddRemove`); the drag in `desktop/src/shelves.rs` (`RowDrag`) comes back with it |
| Delete playlist | menu | yes | `desktop/src/edit_playlist.rs`, after a confirmation |
| Add whole playlist into another | menu | no | |
| Collaborative playlists | `join_collaborative_playlist` | no | |
| Upload songs | `upload_song` | no | |
| Delete uploads | `delete_upload_entity` | no | |
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
| Full screen | `f` | partial | `f` opens the in-window expanded player, not OS full screen |
| Like current song | `+` | yes | `desktop/src/shortcuts.rs` |
| Dislike current song | `_` | yes | `desktop/src/shortcuts.rs` |
| Go to Home, Explore, Library, Settings | `gh`, `ge`, `gl`, `g,` | yes | `desktop/src/shortcuts.rs` (`resolve`), `Ctrl+,` for Settings too |
| Search | `/` | yes | `desktop/src/shortcuts.rs`, also `Ctrl+F` |
| Shortcuts overlay | `?` | yes | `desktop/src/shortcuts.rs` (`overlay`) |
| Back and forward in history | `Alt+Left`, `Alt+Right` | yes | `desktop/src/app.rs` |

## Settings

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Audio quality (Low, Normal, High, Always high) | playback settings | no | kopuzd has no quality setting |
| Premium bitrate | automatic with Premium | yes | kopuzd |
| Autoplay | playback settings | no | kopuzd has no autoplay setting |
| Show or hide music videos (audio only) | playback settings | no | See the Song or Video toggle |
| Restrict explicit content | playback settings | no | kopuzd says nothing of explicit tracks |
| Pause watch history | privacy | no | kopuzd always reports plays when signed in |
| Delete watch history | privacy | no | |
| Notifications | account settings | no | No desktop notifications; in-app toasts only (`desktop/src/toast.rs`) |
| Connected apps, scrobbling | account settings | partial | Last.fm and ListenBrainz (token) in `desktop/src/settings.rs`, sent by kopuzd; no per-service switches |
| Sign in, sign out, brand accounts | avatar menu | yes | `desktop/src/signin.rs` through kopuzd's browser sign-in, profile import and pasted cookies; brand accounts from the account menu |

## Gaps

Most used first.

1. Clear queue and save queue as playlist. ~2 hours.
2. Episodes for Later entry, podcast resume position, episode save. ~a day.
3. Desktop notifications on track change. ~2 hours.
4. Song credits, taste profile, uploads, collaborative playlists. Each ~a day or more; uploads need new protocol work.
