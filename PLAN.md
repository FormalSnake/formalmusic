# FormalMusic: plan

FormalMusic is a YouTube Music client. Binary `formalmusic`, daemon
`formalmusicd`, repo `FormalSnake/formalmusic`, app id `es.canarycoders.formalmusic`.

A GPUI client at full parity with music.youtube.com, on top of our own daemon,
themed by matugen the same way `../messages` is, packaged as a flake with a Home
Manager module, and kept working by a weekly Claude Code maintenance run.

## Architecture

Two binaries, one socket between them.

- **`formalmusicd`**, the daemon. It owns the YouTube session, an InnerTube client
  (WEB_REMIX) for every page and mutation, stream resolution through yt-dlp,
  the audio engine, the queue and MPRIS. It runs as a systemd user service, so
  music keeps playing when the window closes, and media keys work without it.
- **`formalmusic`**, the GPUI app. It's a pure frontend: a store, a bridge and
  screens. Its only I/O is the socket and artwork fetches.

Why our own daemon and not Kopuz: Kopuz's API is generic (catalog, search,
player), so parity would mean maintaining a fork with a YouTube Music service
bolted on. That fork would carry an unstable upstream schema, a Bazel build and
EUPL licensing. Writing the code costs nothing here; the ongoing cost is
YouTube breaking stream extraction, and yt-dlp handles that faster than any
single app's team.

### formalmusicd internals

- **InnerTube client:** `browse`, `next`, `search`, `music/get_search_suggestions`,
  `like/*`, `playlist/*`, `browse/edit_playlist`, `account/accounts_list`.
  Typed parsers per renderer, each tested against a recorded response in
  `crates/innertube/fixtures/`. Responses keep YouTube Music's own shape
  (shelves, chips, header variants), not a flattened track list.
- **Auth:** a sign-in window (GPUI app opens a WebKitGTK login, or the user
  pastes a cookie header in a fallback), with cookies stored in
  `$XDG_STATE_HOME/formalmusicd/session.json` (chmod 600). Brand accounts use the
  `X-Goog-PageId` header. The same cookies go to yt-dlp via `--cookies`, so
  Premium bitrates work.
- **Streams:** `yt-dlp -J` per track, opus first. Resolve the next two queue
  entries ahead of time so the ~1s yt-dlp startup never lands on a track
  change. URLs expire after ~6h, so re-resolve when one is stale.
- **Audio:** symphonia decoding opus/webm and aac/m4a, played through cpal
  over HTTP range reads, with gapless playback and crossfade. Volume
  normalisation uses `loudnessDb` from the player response, which is what
  the web app does.
- **MPRIS:** `mpris-server` crate, including `Rate` and the artwork URL.
- **API:** JSON lines on `$XDG_RUNTIME_DIR/formalmusic/formalmusicd.sock`
  (`crates/api`). Requests carry an id, responses echo it, and `subscribe`
  turns on player, queue and library events. Both ends are Rust and ship in
  one package, so there is no codegen; a shell or bar widget can drive it
  with `socat`.
- **Scrobbling and history:** report playback to YouTube Music's
  `playbackTracking` URLs, so Home recommendations and History stay accurate.
  Without it the account goes stale.

## Workspace

```
crates/innertube     InnerTube requests, renderer parsers, fixtures
crates/formalmusicd          daemon: session, streams, audio, queue, MPRIS, gRPC server
crates/api           wire types and JSON-lines framing shared by both ends
crates/player        streaming decode and audio output
crates/core          app store, StoreEvent, StateCache, socket client
crates/desktop       binary `formalmusic`, gpui-kit 0.6.6, one file per screen
nix/package.nix      both binaries
nix/hm-module.nix    programs.formalmusic
maintenance/         weekly run prompt and live checks
docs/parity.md       checklist against music.youtube.com
```

`core` follows messages' store contract: the UI calls store methods, reads
state under a short `RwLock`, and narrow events (`Page(id)`, `Playlist(id)`,
`Library(tab)`, `Like(video_id)`, `Player`, `Queue`) go through one
`bridge.rs`. Pages paint from `$XDG_CACHE_HOME/formalmusic/state.json` before
the daemon answers.

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

- **yt-dlp:** a flake input pinned to its GitHub release tag, applied as an
  overlay over nixpkgs' derivation. Nixpkgs lags releases by days, and the
  curl-cffi test breakage already worked around in
  `~/.config/nix/modules/shared/mixins/nix.nix` shows that tracking unstable
  is fragile. `formalmusicd` gets the pinned yt-dlp baked into its wrapper `PATH`.
- **`packages.formalmusic`:** builds both binaries for x86_64 and aarch64
  Linux. It copies messages' `package.nix`: the same `patchelf --add-rpath`
  for wayland, vulkan, xkbcommon and X11, plus `alsa-lib` (cpal), and `wrapProgram` putting yt-dlp on `formalmusicd`'s `PATH`.
- **`hm-module.nix`:** `programs.formalmusic.enable` installs the package,
  runs `formalmusicd` as a `systemd.user.services` unit with `Restart=on-failure`,
  registers the matugen template, and never owns `config.json`.
- **Dev shell:** messages' `linuxLibs` plus `socat` and the pinned
  yt-dlp.
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
2. **Update:** bumps the `yt-dlp` and `nixpkgs` flake inputs and runs
   `cargo update`.
3. **Check:** runs `cargo test --workspace` (fixture parsers) and
   `maintenance/live-check.sh`. The live check resolves and decodes 10 seconds
   of a fixed track, fetches Home, Search and one album with the stored
   session, and diffs the renderer keys against the fixtures.
4. **Repair:** fixes whatever fails. Changed YouTube responses get
   re-recorded fixtures and parser fixes. After three distinct failed fix
   attempts it stops and opens a GitHub issue with the log.
5. **Ship through a PR:** commits on a `maintenance/<date>` branch, opens a PR
   with what changed and the check output, and merges it into main with
   `gh pr merge` once step 3 passes. A run that fails step 3 leaves its PR
   open as a draft for you instead. Then `just ui formalmusic` in
   `~/.config/nix`, a rebuild of g815, a push, and the e1504g one-shot rebuild
   from that repo's CLAUDE.md.
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
