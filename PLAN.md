# FormalMusic: plan

FormalMusic is a YouTube Music client. Binary `formalmusic`, daemon `kopuzd`
from FormalSnake/kopuz, repo `FormalSnake/formalmusic`, app id
`es.canarycoders.formalmusic`.

A GPUI client at full parity with music.youtube.com, on top of Kopuz's daemon,
themed by matugen the same way `../messages` is, packaged as a flake with a Home
Manager module, and kept working by a weekly Claude Code maintenance run.

## Architecture

Two binaries, one socket between them.

- **`kopuzd`**, Kopuz's headless daemon, at the rev `Cargo.toml` pins
  `kopuz-client` to (FormalSnake/kopuz `ytm/integration`, where the YouTube
  Music work lands ahead of upstream). It owns the session, the YouTube Music
  source (pages, search, mutations, stream resolution), the audio engine with
  gapless, crossfade, loudness normalisation and the equalizer, the queue,
  lyrics, scrobbling and MPRIS/SMTC. It runs as a systemd user service, so
  music keeps playing when the window closes, and media keys work without it.
- **`formalmusic`**, the GPUI app. It's a pure frontend: a store, a bridge and
  screens. Its only I/O is the socket, artwork through kopuzd, and Apple
  Music motion artwork, which kopuzd has no notion of.

The app sets up a YouTube Music source in kopuzd on first connect and plays
from it. That is the one place it names the service: every other question
("can this be rated, followed, saved, reordered?", "which pages and search
filters are there?") is answered by the source's capabilities.

### The seam

`crates/core/src/kopuz.rs` holds the connection (spawning kopuzd when nothing
answers, reconnecting with backoff, the wire revision handshake) and turns
the store's calls into kopuz API calls. `convert.rs` turns kopuz's wire types
into `model.rs`, the window's own model: YouTube Music's page shapes, serde so
`state.json` can paint them, hashable where they key a cache. Every screen's
data path has a live test in `crates/core/tests/live.rs`.

Where kopuzd lacks something FormalMusic had, the gap is listed in the PR
that moved the app onto it, as an API kopuz needs, rather than worked around
here.

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
crates/core          app store, StoreEvent, StateCache, the kopuzd backend
crates/desktop       binary `formalmusic`, gpui-kit 0.6.6, one file per screen
crates/extras        Apple Music motion artwork
nix/package.nix      the app
nix/kopuzd.nix       kopuzd from the pinned kopuz rev
nix/hm-module.nix    programs.formalmusic
maintenance/         weekly run prompt and live checks
docs/parity.md       checklist against music.youtube.com
```

`core` follows messages' store contract: the UI calls store methods, reads
state under a short `RwLock`, and narrow events (`Page(id)`, `Playlist(id)`,
`Library(tab)`, `Like(video_id)`, `Player`, `Queue`) go through one
`bridge.rs`. Pages paint from `$XDG_CACHE_HOME/formalmusic/state.json` before
kopuzd answers.

## Theming

Copy `messages/crates/desktop/src/theme.rs` and keep the mechanism identical:
a GPUI `Global` palette, `~/.config/formalmusic/theme.json` polled every second
off the UI thread, and `refresh_windows()` on change. Add
`~/.config/nix/users/kyandesutter/matugen-templates/formalmusic.json.tmpl`
with the same Material keys as `messages.json.tmpl`, plus player tokens
(`progress`, `progressTrack`, `nowPlaying`, `scrim`).

Matugen is the only palette source. Album art may nudge the fullscreen player
(a blurred art backdrop under the matugen `scrim`, or a slight hue pull on
`nowPlaying`), kept only if it looks good next to the rest of the desktop;
it never replaces the tokens. Once two apps share it,
`theme.rs` moves into a shared crate.

## Nix

- **kopuz:** a flake input (`flake = false`) at the same rev `Cargo.toml` pins
  `kopuz-client` to; the two move together. Kopuz's own flake packages the
  Dioxus app and not kopuzd, so `nix/kopuzd.nix` builds `kopuz-kopuzd` from
  that source, with the prebuilt librusty_v8 kopuz's packaging also uses.
- **`packages.formalmusic`:** builds the app for x86_64 and aarch64 Linux. It
  copies messages' `package.nix`: the same `patchelf --add-rpath` for
  wayland, vulkan, xkbcommon and X11, and `wrapProgram` putting kopuzd on the
  app's `PATH` so a window with no service still finds the daemon it was
  built against. `packages.kopuzd` is the daemon alone.
- **`hm-module.nix`:** `programs.formalmusic.enable` installs the package,
  runs `kopuzd` as a `systemd.user.services` unit with `Restart=on-failure`,
  registers the matugen template, and never owns `config.json`.
- **Dev shell:** messages' `linuxLibs` plus kopuzd.
- **App id:** `es.canarycoders.formalmusic`, with the same single-instance
  handover as messages.

## Weekly maintenance run

A systemd user timer on the g815 (always on the charger, already the build host
and the one Claude drives the sudo mesh from). `OnCalendar=weekly` with
`Persistent=true`, so a week spent booted into Windows fires the run on the
next NixOS boot. It's declared in `~/.config/nix` next to the claude-code
package from `claude-code-nix`, and starts headless Claude Code with
`maintenance/weekly.md` as the prompt. The run, in order:

1. **Preflight:** checks drift in this repo and `~/.config/nix` on every host,
   following the nix repo's "keep all three hosts in sync" rule. If any tree is
   dirty, it stops and notifies instead of touching anything.
2. **Update:** moves the kopuz pin (`Cargo.toml` and the `kopuz` flake
   input together) to the head of FormalSnake/kopuz `ytm/integration`, bumps
   `nixpkgs` and runs `cargo update`.
3. **Check:** runs `cargo test --workspace` and `maintenance/live-check.sh`,
   which runs the live data-path tests against an isolated, anonymous kopuzd
   of the pinned rev.
4. **Repair:** fixes whatever fails on this side of the seam. YouTube
   breaking stream resolution or a page is kopuz's to fix: the run pins back
   to the last kopuz rev that passed and opens an issue saying what broke.
   After three distinct failed fix attempts it stops and opens a GitHub issue
   with the log.
5. **Ship through a PR:** commits on a `maintenance/<date>` branch, opens a PR
   with what changed and the check output, and merges it into main with
   `gh pr merge` once step 3 passes. A run that fails step 3 leaves its PR
   open as a draft for you instead. Then `just ui formalmusic` in
   `~/.config/nix`, a rebuild of g815, a push, then e1504g's closure built on
   g815 and pushed over with `nixos-rebuild switch --flake .#e1504g
   --target-host e1504g --sudo`. Nothing ever builds on e1504g itself.
6. **Wait for offline hosts:** if the e1504g is unreachable, it retries over
   Tailscale every 30 minutes for up to 48 hours, then gives up and says so.
7. **Report:** sends a desktop notification on g815 with one line per host
   (commit, rebuilt yes or no). The full log goes to the journal.

The same prompt runs on demand (`systemctl --user start formalmusic-maintenance`) when
playback breaks mid-week. The macbook stays out of the run, since the app
targets Linux only.

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
