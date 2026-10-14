# NativeDesktop port: parity with the GPUI window

Every feature of the GPUI window (`crates/desktop`, removed with this port) and of the
daemon around it, and how the nd window (`crates/app`) was checked to keep it.

- **T3**: `crates/app/tests/real.rs`, the built app on the demo catalog over its
  automation socket (`ND_REAL=1 cargo test -p formalmusic --test real`), on AppKit on
  the macbook and in a private headless Hyprland on g815.
- **T1**: `crates/app/tests/tier1.rs` and unit tests (`cargo test -p formalmusic`).
- **Audit**: `crates/app/tests/appearance.rs`, light, dark and matugen on both backends.
- **Run**: the app started by hand on the named host.
- **Code**: kept by construction (the same store call, unchanged daemon code).

| Feature | Verified by |
| --- | --- |
| Home with mood chips, shelves, quick picks, carousels | T3 (Home chips), Run macbook |
| Explore, New releases, Charts, Moods, mood pages | Code: one generic page view over the same `Page` model |
| Search with suggestions, history suggestions, item suggestions | T3 (palette search), Code |
| Search filter chips and "Show all" | T3 (Songs filter) |
| Search correction line | Code |
| Album, artist, playlist, podcast pages with their headers | Run macbook, Code |
| Header Play, Shuffle, Radio, Subscribe, Save to library, Edit playlist | Code |
| Description clamp with More / Less | Code |
| Library tabs (playlists, songs, albums, artists, subscriptions, podcasts, uploads) and History | Run macbook (demo Library), Code |
| Library loads in full: a dead session answers "Sign in", not an empty page | innertube `library_sign_in_prompt_is_signed_out` (recorded responses) |
| Pages and shelves load more as they scroll (continuations) | Code: `on_near_end` and per-shelf rows; innertube continuation fixtures |
| Pages paint from `state.json` before the daemon answers | Code (core unchanged) |
| Back and forward, history kept per page with scroll position | T3 (back enabled after search), Code |
| Prefetch of the first cards on Home and Explore | Code |
| Sidebar: Home, Explore, Library, History, New playlist, your playlists | Run macbook |
| Player bar: play/pause, previous, next, seek, buffered range, volume, mute | T3 (seek), Run macbook |
| Player bar: like, dislike, repeat, shuffle, open player | Run macbook, Code |
| Motion artwork in the bar (setting) and expanded player | Code (`video.rs` over core's ffmpeg `Loop`) |
| Expanded player: cover, Song/Video switch, Up next, Lyrics, Related | T3 (Lyrics, Up next), Audit (lyrics window) |
| Music video in Video mode, held to the audio clock | Code (`video.rs` over core's `Synced`) |
| Synced lyrics: word fill, background and duet lines, interludes | T1 (`convert`), nd lyrics model |
| Lyrics follow playback, follow a seek, user scroll takes over, resync | T3 (seek and play), nd lyrics view |
| Click a lyric line to seek | Code |
| Queue: jump, remove, reorder | T1, Code (menu Move up/down; see gaps) |
| Context menus: radio, play next, add to queue, add to playlist, save, remove, like, dislike, go to album or artist, share | T1 (`a_track_menu_queues_and_rates_the_track`, `share_copies_and_go_to_opens`) |
| Playlist editing: rename, describe, privacy, delete, reorder rows | Code (edit sheet, Move up/down) |
| New playlist | Code |
| Sign in: browser profile import, new browser window, cookie paste, browse signed out | Code |
| Account switching, sign out | Code (Account menu) |
| Settings: quality, autoplay, explicit, normalisation, crossfade, equalizer, history, keep playing, tray, bar cover | api `old_json_files_become_config_toml`, daemon builds against `config.toml` |
| Scrobbling: Last.fm and ListenBrainz connect, switches, queued count | Code |
| Notices as toasts that clear themselves | Code |
| Offline banner while the daemon is unreachable | Code |
| Single-key shortcuts and chords (`?` lists them) | T3 (`q`, Escape) |
| Single instance: a second launch raises the window | Code (nd-ipc instance) |
| Close pauses unless "keep playing" | Code |
| Theme from matugen, light and dark | Audit |
| MPRIS with shuffle, loop, volume, Seeked; tray; Windows SMTC | Code (daemon unchanged) |
| Demo mode (`FORMALMUSIC_DEMO=1`) | T3 |
| Packaging: id `dev.formalsnake.formalmusic`, desktop entry, icon | `nativedesktop.toml`, `nix/package.nix` |
| Home Manager module with typed settings | api `home_manager_options_are_current` |
| Data migration from `config.json` and `daemon.json` | api `old_json_files_become_config_toml` |

## Status, 2026-10-14

Not ready to become the default. Checked on the macbook: the app builds and runs on AppKit,
the tier 3 test passes (search, play, seek, lyrics follow, keys, chords, settings window),
the appearance audit passes on AppKit and headless GPUI, tier 1 and the library regression
test pass. Open before the switch to main:

- [ ] Linux leg on g815: build, `cargo test` for the workspace, tier 3 in headless Hyprland
      (stopped for the build budget; the daemon's ksni now uses async-io to build beside
      nd-system, unverified).
- [ ] `nix build .#formalmusic` once (nativedesktop pinned to 94f2cbca in `flake.lock`;
      `libghostty-vt-0.2.1` in `outputHashes` still needs its real hash).
- [ ] A run against a real daemon with a signed-in session (the g815 session is dead at
      the source: the Helium profile's YouTube cookies are rotated out, so the library
      shows the sign-in page until the user signs in again).
- [ ] Rows marked "Code" above checked by hand on a real session.

Known losses against the GPUI window, all waiting on nd:
icon-only rating, repeat and shuffle buttons (text until `Icon` gains media glyphs), drag
to reorder (Move up/down menu rows instead), hover prefetch (no hover event on containers),
"More actions" buttons (right-click only), macOS Now Playing (the daemon has no AppKit run
loop; MPRIS and SMTC stay in the daemon since nd-system's session lacks shuffle, loop,
volume and Seeked).
