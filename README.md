<p align="center"><img src="packaging/linux/es.canarycoders.formalmusic.png" width="160" alt="FormalMusic icon"></p>

# FormalMusic

YouTube Music on Linux and Windows. A native Rust client (GPUI) on top of
[Kopuz](https://github.com/FormalSnake/kopuz)'s playback daemon, `kopuzd`,
with synced lyrics and Apple Music motion artwork.

![Home](docs/images/home.png)

```
┌──────────────────────┐  gRPC, kopuz-client  ┌────────────────────────────────┐
│ formalmusic (GPUI)   │ ───────────────────▶ │ kopuzd                         │
│ window, Vulkan, tray │ ◀─────────────────── │ YouTube Music source, streams, │
└──────────────────────┘  unix socket or pipe │ audio engine, queue, MPRIS     │
                                              └────────────────────────────────┘
```

The daemon owns playback, so music keeps going with the window closed and media
keys work through MPRIS. The window paints from its last state before the
daemon answers, pages you have seen open in the frame you click, and it idles
at 0% CPU. FormalMusic sets up a YouTube Music source in kopuzd the first time
it connects and plays from it; everything past that comes from what the source
says it can do.

| | |
|---|---|
| ![Album](docs/images/album.png) | ![Lyrics](docs/images/lyrics.png) |
| ![Search](docs/images/search.png) | ![Queue](docs/images/queue.png) |

## Features

- Home with mood chips, Explore (new releases, charts, moods and genres),
  search with suggestions and filters, album, artist, playlist and podcast pages.
- Library: playlists, songs, albums, artists, subscriptions, podcasts, uploads,
  liked music and history. Likes, subscriptions and playlist editing.
- Queue with drag to reorder, play next, radio that keeps itself topped up,
  shuffle and repeat. Gapless playback, optional crossfade, loudness
  normalisation from YouTube's own values, a ten band equalizer.
- Premium streams when the account has Premium.
- Word-synced lyrics from Apple Music, YouTube Music or LRCLIB, drawn in the
  Apple Music style.
- Apple Music motion artwork for albums that have it.
- Plays reported to YouTube so History and recommendations stay current.
- Colours follow [matugen](https://github.com/InioX/matugen) live.

Video playback and comments are not there yet.

## Install

### Nix

```
nix run github:FormalSnake/formalmusic
```

Home Manager:

```nix
{
  inputs.formalmusic = {
    url = "github:FormalSnake/formalmusic";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  # in your home-manager configuration
  imports = [ inputs.formalmusic.homeModules.default ];

  programs.formalmusic = {
    enable = true;
    daemon = true;                     # kopuzd as a systemd user service
    theme = { accent = "#6099c0"; };   # optional, writes theme.json
  };
}
```

This installs the app, the desktop entry and icon, and `kopuzd` built from the
kopuz rev `Cargo.toml` pins `kopuz-client` to. `overlays.default` adds
`pkgs.formalmusic` and `pkgs.kopuzd`.

### Other Linux

Needs a recent stable Rust, Vulkan, `ffmpeg` on `PATH`, and `libxkbcommon`,
`wayland`, `vulkan-loader`, `fontconfig` and `freetype`; kopuzd also needs
`alsa-lib` and `libopus`.

```
git clone https://github.com/FormalSnake/formalmusic
cd formalmusic
cargo build --release -p formalmusic
SQLX_OFFLINE=true cargo install --locked --git https://github.com/FormalSnake/kopuz.git \
  --rev "$(grep -oP 'kopuz.git", rev = "\K[0-9a-f]+' Cargo.toml | head -1)" kopuz-kopuzd
```

Put both binaries on `PATH`. The app starts kopuzd itself when no
`kopuzd.service` is running.

### Windows

Needs Rust (MSVC) and the Visual Studio C++ build tools. From the checkout:

```
powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1
```

This builds the app and kopuzd and installs them for the current user in
`%LOCALAPPDATA%\Programs\FormalMusic`, with a Start menu entry and an entry
in Installed apps. ffmpeg comes from winget when it is not on `PATH`. kopuzd
shows up in the media flyout and on media keys and talks over its per-user
named pipe; the app keeps a notification area icon while a track is loaded.

## Signing in

On first launch, pick a browser profile that is already signed in to YouTube
Music and kopuzd copies its session, which works while the browser is open.
Otherwise it opens your browser at Google's sign-in page and keeps the
cookies once you are in. Pasting the `Cookie` request header of a signed-in
music.youtube.com tab still works as a last resort. kopuzd keeps the session
in its own database. Without signing in, everything that doesn't need an
account works.

`FORMALMUSIC_DEMO=1 formalmusic` runs on a made-up catalog with no daemon.

## Scrobbling

Settings (`Ctrl+,`, or the account menu) connects Last.fm and ListenBrainz;
kopuzd sends the scrobbles.

Last.fm signs every call with an API account, so you need your own: create
one at [last.fm/api/account/create](https://www.last.fm/api/account/create)
(no callback URL), paste its API key and shared secret into Settings, then
allow access in the browser. With Home Manager the pair can come from files
instead, such as agenix secrets:

```nix
programs.formalmusic.lastfm = {
  apiKeyFile = "/run/agenix/lastfm-api-key";
  sharedSecretFile = "/run/agenix/lastfm-shared-secret";
};
```

ListenBrainz connects with the user token from
[listenbrainz.org/settings](https://listenbrainz.org/settings/). kopuzd keeps
the session keys and tokens.

## Configuration

`~/.config/formalmusic/config.json` holds the Settings dialog's switches and
the equalizer, plus two keys it has no switch for. The app hands the audio
ones to kopuzd on every connect:

```json
{ "normalisation": true, "crossfadeMs": 0 }
```

`~/.config/formalmusic/theme.json` overrides palette tokens (fields of
`PaletteFile` in `crates/desktop/src/live_theme.rs`) and is reloaded live, so
matugen can drive it:

```json
{ "canvas": "#1c1917", "text": "#b4bdc3", "accent": "#6099c0" }
```

Environment overrides: `FORMALMUSIC_SOCKET` (kopuzd's socket, by default
`$XDG_RUNTIME_DIR/kopuz/kopuzd.sock`), `FORMALMUSIC_FONT`,
`FORMALMUSIC_TRACE=1` (startup and navigation timings).

kopuzd serves gRPC with reflection on, so a bar widget can drive it:

```
grpcurl -unix -plaintext $XDG_RUNTIME_DIR/kopuz/kopuzd.sock kopuz.v1.Kopuz/Toggle
```

## Keyboard

| | |
|---|---|
| `Space` / `;` | play or pause |
| `J` / `K`, `Shift+N` / `Shift+P`, `N` / `P` | next / previous |
| `H` / `L`, `Shift+←` / `Shift+→` | back / forward 10 seconds |
| `Shift+H` / `Shift+L` | back / forward 1 second |
| `=` / `-`, `Shift+↑` / `Shift+↓` | volume |
| `M` | mute |
| `+` / `_` | like / dislike |
| `S` / `R` | shuffle / repeat |
| `G H`, `G E`, `G L`, `G ,` | Home, Explore, Library, Settings |
| `/`, `Ctrl+F` | search |
| `Q` | queue |
| `F` | expanded player |
| `?` | all shortcuts |
| `Alt+←` / `Alt+→` | back / forward |
| `Ctrl+,` | settings |

`Cmd` on macOS.

## Development

```
nix develop
FORMALMUSIC_DEMO=1 cargo run --release -p formalmusic   # made-up catalog
cargo test --workspace
FORMALMUSIC_SOCKET=<socket> cargo test -p formalmusic-core --test live -- --ignored --test-threads 1
nix build .#formalmusic                                 # Linux package
```

The live tests walk every screen's data path, playback and the library
changes (each undone) against a running kopuzd. `maintenance/live-check.sh`
runs them against an isolated, anonymous kopuzd of the pinned rev. A weekly
systemd timer on the maintainer's machine runs Claude Code headless with
`maintenance/weekly.md` as the prompt: it bumps the kopuz pin, nixpkgs and the
crates, runs the checks, repairs what broke and ships the result as a PR.

`crates/core` is the window's store: it talks to kopuzd through
`kopuz-client` and turns kopuz's wire types into the model the window draws
(`model.rs`, `convert.rs`). `crates/desktop` is the window, and
`crates/extras` looks up Apple Music motion artwork, which kopuzd has no
notion of. `PLAN.md` has the architecture and the speed budget.

## License

[MIT](LICENSE). The fonts in `packaging/fonts` are under the
[OFL](packaging/fonts/OFL.txt). Not affiliated with YouTube, Google or Apple.
