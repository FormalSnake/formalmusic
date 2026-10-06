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
| Episodes for Later | `SE` playlist in sidebar | no | No entry; the playlist would browse generically |
| Search results | `/search?q=` | yes | `desktop/src/topbar.rs`, `desktop/src/page.rs` |
| Search suggestions and history | dropdown | yes | `desktop/src/topbar.rs` (`Suggestion::Query`, `from_history`) |
| Search filters | Songs, Videos, Albums, Featured playlists, Community playlists, Artists, Podcasts, Episodes, Profiles | partial | UI has 7 of 9 (`desktop/src/page.rs`); Episodes and Profiles exist in `api/src/model.rs` only |
| Search within library | library search | partial | `SearchFilter::Library` exists, no UI |
| Delete a search history entry | x on suggestion | no | ytmusicapi `remove_search_suggestions`, not in `api` |
| Search top result card | Top result | yes | `desktop/src/shelves.rs` |
| Album page | `/browse/MPREb...` | yes | `desktop/src/header.rs`, `desktop/src/page.rs` |
| Artist page | `/channel/UC...` | yes | `desktop/src/header.rs` (`Header::Artist`) |
| Artist "see all" shelves | More | yes | `desktop/src/shelves.rs` (`ArtistShelf`) |
| Playlist page | `/playlist?list=` | yes | `desktop/src/header.rs`, `desktop/src/page.rs` |
| Podcast page | `/podcast/` | partial | Browses and lists episodes; no follow button of its own beyond `Save to library` |
| Episode page | `/episode/` | partial | `BrowseTarget::Episode` browses, plays as `TrackKind::Episode`; no resume position, no "save for later" |
| Channel and user pages | `get_user`, `get_channel` | partial | Reached only as artist or podcast links; no Profiles search |
| Watch page, Up next | `/watch` | yes | `desktop/src/now_playing.rs` (`QueueView`) |
| Watch page, Lyrics | tab | yes | `desktop/src/lyrics.rs`, `extras/src/lyrics/`; word-synced, beyond the web app |
| Watch page, Related | tab | yes | `desktop/src/now_playing.rs`, `core/src/store.rs` (`load_related`) |
| Song credits | `get_song_credits` | no | |
| Taste profile (pick artists) | `get_tasteprofile` | no | |
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
| Clear queue | button | partial | `Command::ClearQueue` in `api/src/lib.rs`, no control in `desktop/src` |
| Save queue as playlist | menu | no | |
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
| Save song to library | track menu | no | ytmusicapi `edit_song_library_status`; only albums and playlists save |
| Save album or playlist to library | menu and header | yes | `desktop/src/actions.rs`, `desktop/src/header.rs` |
| Like, dislike from menu | track menu | yes | `desktop/src/actions.rs` |
| Go to album, go to artist | track menu | yes | `desktop/src/actions.rs` |
| Remove from queue | queue menu | yes | `desktop/src/now_playing.rs` |
| Remove from history | history menu | yes | `desktop/src/actions.rs` (`track_menu`), `core/src/store.rs` (`remove_from_history`) |
| Share, copy link | menu | no | No clipboard or share code outside sign-in |
| Report, not interested | menu | no | |
| Shuffle play an album or playlist | menu | yes | `desktop/src/actions.rs` |
| Subscribe to an artist | artist header | yes | `desktop/src/header.rs`, `daemon/src/daemon.rs` |
| Hover play button on cards | cards | yes | `desktop/src/shelves.rs`, `desktop/src/actions.rs` (`play_item`) |

## Playlists and library edits

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Create playlist | New playlist | partial | `desktop/src/new_playlist.rs` takes a title only; privacy and description exist in `Command::CreatePlaylist` |
| Rename, describe, change privacy | edit playlist | partial | `PlaylistEdit::{Rename,Describe,SetPrivacy}` in `api/src/model.rs`, no UI |
| Reorder tracks in a playlist | drag | partial | `PlaylistEdit::Move`, no UI |
| Delete playlist | menu | partial | `Command::DeletePlaylist`, no UI |
| Add whole playlist into another | menu | partial | `PlaylistEdit::AddPlaylist`, no UI |
| Collaborative playlists | `join_collaborative_playlist` | no | |
| Upload songs | `upload_song` | no | |
| Delete uploads | `delete_upload_entity` | no | |
| Subscribe to a podcast | podcast page | partial | Via `Save to library` where the header carries a playlist id |

## Keyboard shortcuts

Read from the web app's `?` overlay. FormalMusic's single keys are in
`desktop/src/app.rs` (`on_key_down`), the chords in the same file's `bind_keys`.

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Play or pause | `Space`, `;` | partial | `Space` and `k` work; `;` does not |
| Next song | `j`, `Shift+n` | partial | `n` and `N` work; `j` seeks back instead |
| Previous song | `k`, `Shift+p` | partial | `p` and `P` work; `k` toggles play instead |
| Forward 10s | `l`, `Shift+Right` | yes | `desktop/src/app.rs` |
| Back 10s | `h`, `Shift+Left` | yes | `desktop/src/app.rs` (`j` too, which the web uses for next) |
| Forward or back 1s | `Shift+l`, `Shift+h` | no | |
| Shuffle | `s` | yes | `desktop/src/app.rs` |
| Toggle repeat | `r` | yes | `desktop/src/app.rs` |
| Volume up, down | `=`, `-` | yes | `desktop/src/app.rs` (`+` and `Shift+Up` too) |
| Mute | `m` | yes | `desktop/src/app.rs` |
| Toggle queue or expanded player | `q`, `Esc` | yes | `desktop/src/app.rs` |
| Full screen | `f` | partial | `f` opens the in-window expanded player, not OS full screen |
| Like current song | `+` | no | `+` raises volume here |
| Dislike current song | `_` | no | |
| Go to Home, Explore, Library, Settings | `gh`, `ge`, `gl`, `g,` | partial | Only `Ctrl+,` for Settings; no `g` chords |
| Search | `/` | yes | `desktop/src/app.rs`, also `Ctrl+F` |
| Shortcuts overlay | `?` | no | Shortcuts are listed in `README.md` only |
| Back and forward in history | `Alt+Left`, `Alt+Right` | yes | `desktop/src/app.rs` |

## Settings

| Feature | Web app | FormalMusic | Notes |
|---|---|---|---|
| Audio quality (Low, Normal, High, Always high) | playback settings | partial | `daemon/src/config.rs` (`preferred_quality`), `daemon/src/streams.rs`; `daemon.json` only, no UI |
| Premium bitrate | automatic with Premium | yes | `daemon/src/streams.rs` |
| Autoplay | playback settings | partial | Always on, no switch |
| Show or hide music videos (audio only) | playback settings | yes | Song/Video switch in `desktop/src/music_video.rs`; no persistent default |
| Restrict explicit content | playback settings | no | |
| Pause watch history | privacy | partial | `reportHistory` in `daemon/src/config.rs`, file only |
| Delete watch history | privacy | no | |
| Notifications | account settings | no | No desktop notifications; in-app toasts only (`desktop/src/toast.rs`) |
| Connected apps, scrobbling | account settings | yes | Last.fm and ListenBrainz, `desktop/src/settings.rs`, `daemon/src/scrobble/` |
| Sign in, sign out, brand accounts | avatar menu | yes | `desktop/src/signin.rs`, `daemon/src/signin/` |

## Gaps

Most used first.

3. Match the web's playback keys: `j`/`k` as next/previous, `;` for play, `+`/`_` for like and dislike. Needs a decision on the current `j`/`k` seek habit. ~1 hour.
4. Playlist editing UI: rename, description, privacy, delete, reorder (all in `PlaylistEdit`). ~a day.
5. Save a single song to the library (`edit_song_library_status`): new command, parser state, menu row. ~half a day.
6. Quality, autoplay, explicit filter and pause history in the Settings dialog, writing `daemon.json`. ~half a day.
7. `g` chords and a `?` shortcuts overlay. ~3 hours.
8. Search filters Episodes and Profiles, plus library search. ~2 hours.
9. Clear queue and save queue as playlist. ~2 hours.
10. Episodes for Later entry, podcast resume position, episode save. ~a day.
11. Share and copy link, desktop notifications on track change. ~half a day.
12. Song credits, taste profile, uploads, collaborative playlists. Each ~a day or more; uploads need new protocol work.
