# FormalMusic: plan

FormalMusic is a YouTube Music client. Binary `formalmusic`, repo
`FormalStudio/formalmusic`, app id `dev.formalsnake.formalmusic`. The daemon is
Kopuz's `kopuzd` (`github:Kopuz-org/kopuz`).

A NativeDesktop client (AppKit on macOS, GPUI elsewhere) at full parity with
music.youtube.com, on top of the Kopuz daemon, themed by matugen the same way
`../messages` is, packaged as a flake with a Home Manager module, and kept
working by a weekly Claude Code maintenance run.

## Architecture

Two processes, one socket between them.

- **`kopuzd`**, Kopuz's daemon, or the Kopuz desktop app, which runs the same
  core in-process and serves the same socket. It owns the YouTube Music
  session, the InnerTube client, stream resolution, the audio engine, the
  queue, lyrics, scrobbling and MPRIS/Now Playing. Music keeps playing when
  the window closes.
- **`formalmusic`**, the window, on NativeDesktop. A pure frontend: a store,
  a bridge and screens. Its only I/O is the socket, plus Apple Music animated
  covers, which it fetches itself.

Kopuz used to be the wrong base: its API was generic (catalog, search,
player), so parity meant a fork with a YouTube Music service bolted on. Since
October 2026 the YouTube Music work that `formalmusicd` carried is upstream in
Kopuz (pages with shelves, chips and header variants, search filters and
suggestions, rate, follow, save, remove from history, brand accounts, song and
video versions, playback tracking, loudness, autoplay radio), so the window
talks to Kopuz and `formalmusicd` is gone.

### The bridge to Kopuz

The store and screens keep the app's own model (`crates/api`): YouTube
Music's page shapes, which Kopuz's catalog carries too. `crates/core/src/kopuz`
is a `Transport` like the demo: each store `Command` becomes the Kopuz calls
that answer it, and Kopuz's `Subscribe` stream becomes the store's events.

- **Wire:** gRPC over the socket Kopuz serves (`~/Library/Caches/kopuz/kopuzd.sock`
  on macOS, `$XDG_RUNTIME_DIR/kopuz/kopuzd.sock` on Linux,
  `FORMALMUSIC_KOPUZ_SOCKET` overrides), through Kopuz's own `kopuz-client`
  crate. The schema is not a stable API, so `Cargo.toml` pins the client
  crates and `flake.nix` pins the daemon to the same Kopuz release (v0.22.0,
  the first with the YouTube Music work), and the window refuses a daemon on
  another wire revision. Only the local socket: kopuzd's `--listen` TCP has
  no authentication since v0.22.0, so nothing here uses or exposes it.
- **Source:** Kopuz has one active source. The window selects the YouTube
  Music one when another is active, and creates one when there is none at
  sign-in.
- **Ids:** a track key is the video id; a page id is `browseId` or
  `browseId?params`. They go into the model's id fields unchanged, so
  `BrowseTarget`s map onto `GetCatalogDetail` one to one (Home, Explore,
  charts, moods, the library tabs and History are pages by their `FEmusic_*`
  ids, Liked music is the `LM` playlist).
- **Artwork:** Kopuz serves bytes, not URLs. A thumbnail URL is
  `kopuz:<kind>/<id>@<version>`, which the art cache fetches with
  `GetArtwork` and the window paints from disk.
- **Row actions:** rate, save and follow refs are opaque. The bridge remembers
  the ones each page handed out by the id the model carries. A playlist the
  account owns gets each row's position as `set_video_id`, which Kopuz's
  remove and reorder take.
- **Settings:** the playback keys of `config.toml` (quality, autoplay,
  explicit, normalisation, crossfade, equalizer, history) are pushed into
  Kopuz's settings on connect and whenever the file changes, one edit per key
  that differs, so a key Kopuz's managed settings pin is left alone.

### Maps one to one

Home with chips and continuations, Explore, New releases, Charts, Moods and
genres, mood pages, album, artist (header, shelves, "see all" pages),
playlist (paged rows), podcast and episode pages, History, every library tab,
search with filters and corrections, suggestions, related, lyrics (synced and
word-timed), like and dislike, subscribe, save albums and playlists, save
songs, remove from history, create, rename, describe, set privacy, add,
remove, reorder and delete playlists, play a list, an album, a playlist or a
track radio, play next, add to queue, jump, move, remove, clear upcoming,
transport, volume, mute, repeat, shuffle, the song or video version of the
playing track, sign-in in a browser, from a browser profile or with pasted
cookies, sign-out, brand accounts, Last.fm and ListenBrainz connections.

### What formalmusicd did that Kopuz lacks

Upstream PR candidates, in the order parity needs them:

1. **Row actions on `TrackInfo`.** Album and playlist rows come as
   `TrackInfo` without `CatalogActions`, so their like state comes from the
   favorites table and their "save to library" toggle is missing. The bridge
   rates a row by its key, which only works because Kopuz's YouTube Music
   rate ref for a video happens to be its id.
2. **Music video picture.** `GetVideo` serves the stream as chunks over gRPC;
   the window's player decodes a URL with ffmpeg. Either Kopuz hands out a
   URL (with its headers), or the window grows a local range proxy over
   `GetVideo`. Until then the Video mode plays the video's audio with the
   cover.
3. **One version for the whole queue.** formalmusicd played every queued
   track as its song or its video, as the web app does; Kopuz's `SetVersion`
   swaps the playing track only.
4. **Premium.** Nothing says whether the account has Premium, which the
   window used for its quality hint.
5. **Cancel a browser sign-in.** `AuthenticateSource` runs until the user
   finishes or it times out; there is no call to close the browser early.
6. **Scrobbler details.** Kopuz says whether Last.fm and ListenBrainz are
   connected, not as whom, and has no per-service scrobble and now-playing
   switches or a count of queued plays.
7. **Shared segments.** formalmusicd mapped positions between a song and its
   video through the stretches both play; Kopuz maps them itself on
   `SetVersion` and does not send them, so the window cannot show where the
   other version would land.
8. **Start before the whole playlist is fetched.** A `Playlist` queue context
   walks every page before playing; formalmusicd started on the first page.
9. **Tray.** formalmusicd drew the Linux tray; kopuzd has none, so the
   `show_in_tray` setting is gone.

## Speed budget

The bar is Spotifast (Rust and egui, opens in well under a second, 100 to
250 MB). FormalMusic has to beat it, measured on g815 and on e1504g:

| | g815 | e1504g |
|---|---|---|
| Cold start to cached Home painted | < 300 ms | < 700 ms |
| Click to a cached page | same frame | same frame |
| RSS after Home, artist, 1000-track playlist, player | < 120 MB | < 150 MB |
| CPU paused and unfocused | 0% | 0% |

The window paints from `state.json` before the socket connects, pages are
stale-while-revalidate, cards prefetch on hover, and images decode at display
size into a byte-bounded LRU. `FORMALMUSIC_TRACE=1` logs startup and
navigation timings so the weekly run can catch regressions.

## Workspace

```
crates/api           the app's model, the store's commands, settings
crates/core          app store, StoreEvent, StateCache, the Kopuz bridge, demo
crates/extras        Apple Music animated covers
crates/app           binary `formalmusic` on NativeDesktop (../nativedesktop)
nix/package.nix      the window, with kopuzd on its PATH
nix/hm-module.nix    programs.formalmusic, kopuzd as a user service
maintenance/         weekly run prompt
docs/parity.md       checklist against music.youtube.com
```

`core` follows messages' store contract: the UI calls store methods, reads
state under a short `RwLock`, and narrow events (`Page(id)`, `Playlist(id)`,
`Library(tab)`, `Like(video_id)`, `Player`, `Queue`) go through one
`bridge.rs`. Pages paint from `$XDG_CACHE_HOME/formalmusic/state.json` before
the daemon answers.

## Theming

NativeDesktop's theme: its tokens, the OS appearance and accent, and on Linux
the matugen `~/.config/formalmusic/theme.json` (nd's template, Material roles)
reloaded live. The app sets no colours of its own; the appearance audit
(`crates/app/tests/appearance.rs`) fails on any colour that is not a token.

## Nix

- **kopuz:** a flake input pinned to the same release as the `kopuz-*` crates
  in `Cargo.toml`, not following our nixpkgs, so kopuz.cachix.org's builds
  still hit. Bumping Kopuz means bumping both together.
- **`packages.formalmusic`:** builds the window for x86_64 and aarch64 Linux.
  It copies messages' `package.nix`: the same `patchelf --add-rpath` for
  wayland, vulkan, xkbcommon and X11, and `wrapProgram` putting ffmpeg and
  kopuz's `kopuzd` on `formalmusic`'s `PATH`.
- **`hm-module.nix`:** `programs.formalmusic.enable` installs the package,
  runs `kopuzd` as a `systemd.user.services` unit with `Restart=on-failure`
  (`daemon = false` for someone who runs the Kopuz app instead; the two own
  one library and cannot run together), registers the matugen template, and
  never owns `config.toml`.
- **Dev shell:** messages' `linuxLibs`, kopuz and grpcurl.
- **App id:** `dev.formalsnake.formalmusic`, with the same single-instance
  handover as messages.

## Weekly maintenance run

A systemd user timer on the g815 (always on the charger, already the build host
and the one Claude drives the sudo mesh from). `OnCalendar=weekly` with
`Persistent=true`, so a week spent booted into Windows fires the run on the
next NixOS boot. It's declared in `~/.config/nix` next to the claude-code
package from `claude-code-nix`, and starts headless Claude Code with
`maintenance/weekly.md` as the prompt. Stream extraction is Kopuz's to keep
working now, so the run keeps FormalMusic on a current Kopuz:

1. **Preflight:** checks drift in this repo and `~/.config/nix` on every host.
   If any tree is dirty, it stops and notifies instead of touching anything.
2. **Update:** moves the `kopuz` flake input and the `kopuz-*` tags in
   `Cargo.toml` to Kopuz's latest release together, bumps `nixpkgs` and runs
   `cargo update`.
3. **Check:** `cargo test --workspace`, then the window against a scratch
   kopuzd (its own `--db-path` and `--socket`, an anonymous YouTube Music
   source): Home, a search, an album, and ten seconds of playback.
4. **Repair:** fixes what a Kopuz schema change broke in `crates/core/src/kopuz`.
   After three distinct failed fix attempts it stops and opens a GitHub issue
   with the log.
5. **Ship through a PR**, rebuild and report as before: a `maintenance/<date>`
   branch, merged once step 3 passes, then `just ui formalmusic` in
   `~/.config/nix`, g815 rebuilt, e1504g's closure built on g815 and pushed
   over, one notification line per host.

## Phases

| # | Scope | Size |
|---|---|---|
| 1 | Workspace, flake with pinned yt-dlp, `formalmusicd` playing a video id end to end with MPRIS, `theme.rs` and matugen, a now-playing bar | 3 days |
| 2 | Sign-in, cookies shared with yt-dlp, Home (chips, shelves), Search with suggestions and filters | 1 week |
| 3 | Album, artist and playlist pages, queue panel, up next, radio, related, lyrics, playback tracking | 1 week |
| 4 | Library tabs, likes and dislikes, add to playlist, playlist create/edit/reorder, history | 1 week |
| 5 | Explore (new releases, charts, moods and genres), podcasts, uploads, brand accounts | 1 week |
| 6 | Fullscreen player, gapless and crossfade, the web app's keyboard shortcuts, notifications, HM module polish | 4 days |
| 7 | Weekly maintenance timer, `live-check.sh`, `weekly.md`, one forced-failure dry run | 2 days |

`docs/parity.md` gets written in Phase 1 by walking every route on
music.youtube.com. A phase closes only when its rows are ticked.
