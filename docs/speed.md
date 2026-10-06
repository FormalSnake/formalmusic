# Speed: what Spotifast does and what FormalMusic should take

Sources: `crmne/spotifast` v0.12.0 (`S/`, cloned to `~/Developer/spotifast-ref`) and
`crmne/fastframe` v0.4.1 (`F/`, `~/Developer/fastframe-ref`). FormalMusic paths are
relative to this repo, read on 2026-10-06 while another agent had uncommitted edits
in `crates/`, so line numbers will drift.

## Measured numbers in Spotifast

There are no benchmarks, benches or timing logs in either repo. The only figures:

| Claim | Where |
|---|---|
| "100 to 250 MB of RAM, starts in well under a second" | `S/README.md:5-6`, `S/docs/_guide/what-is-spotifast.md:10-12` |
| Library grid loads 300 px covers instead of full size, to cut memory | `S/packaging/release-notes/v0.10.1.md:13` |
| rodio kept the paused device stream running, about 1% of a Mac core | `F/crates/fastframe-audio/README.md:8-11` |
| "Startup time, idle work, memory use, and binary size are product features" | `S/CONTRIBUTING.md:68-69` |

FormalMusic's `FORMALMUSIC_TRACE=1` (`crates/desktop/src/trace.rs`) already measures
more than Spotifast does.

## How Spotifast is fast

| Area | What it does | Where |
|---|---|---|
| Startup | `main` runs settings load, single-instance lock and `App::new` on the main thread, then `eframe::run_native`. The backend (tokio, 2 workers) starts on its own thread, so nothing in it blocks the first frame | `S/src/entrypoint.rs:337-574`, `S/src/backend.rs:915-937` |
| Deferred init | Proxy credentials come from the keyring asynchronously, with commands queued until then. librespot connects only after playback is authorised. Emoji fonts are found on a thread | `S/src/backend.rs:2034-2042`, `:2497-2505`, `S/src/emoji.rs:20` |
| UI state on disk | Only the session (page, recents, resume point, sorts). There is no API snapshot painted before the network answers | `S/src/app.rs:698`, `:9931-9937` |
| Playlist disk cache | One JSON per playlist, valid for exactly one snapshot id, written incrementally and read with `spawn_blocking` | `S/src/backend.rs:4018-4030`, `:3168-3200`, `:4252-4310` |
| Fonts | System fallback faces found once (`OnceLock`) and memory-mapped, so only the pages epaint reads are loaded. `fc-match` capped at 1 s | `F/crates/fastframe-fonts/src/system.rs:108-120`, `:212-217` |
| Threading | UI to backend on a tokio unbounded mpsc, backend to UI on std mpsc drained each frame. Workers wake the UI through a `Waker` that is a no-op once the window is gone | `S/src/backend.rs:913-914`, `:1222`, `F/crates/fastframe-shell/src/lib.rs:326-343` |
| Idle CPU | One scheduler asks for the next frame: 250 ms while playing, 120 ms while a play is pending, 4 s or 20 s while connected, nothing otherwise | `S/src/app.rs:9697-9712`, `:33-34` |
| Progress | librespot reports position once a second; the UI interpolates from `position_at: Instant` | `S/src/player.rs:344`, `:216-226` |
| Audio idle | Own cpal stream, paused on pause, so a paused player gets no callbacks | `F/crates/fastframe-audio/README.md:6-12`, `S/src/sink.rs:767` |
| Audio buffer | 100 ms device buffer (20 to 500 allowed), gapless and preload left to librespot | `S/src/sink.rs:57-63`, `S/src/player.rs:339-349` |
| HTTP | One shared reqwest client (rustls, gzip, `http2`) for API, art, lyrics and updates | `S/src/http.rs:12-71`, `S/Cargo.toml:133` |
| API limits | 6 requests in flight per client, a second semaphore of 4 for background work, shared 429 cooldown honouring `Retry-After` up to 30 s | `S/src/api/client.rs:23-25`, `:388-401`, `:477-511`, `S/src/backend.rs:1404` |
| Image memory | Byte-capped at 64 MiB counting decoded plus texture bytes; JPEG bytes dropped once the texture exists; oldest-used evicted on a 20 s tick; 8 MiB max payload | `S/src/images.rs:18-25`, `:165`, `:207`, `S/src/app.rs:2831-2834` |
| Page memory | In-memory pages capped per kind (12 playlists, 16 albums, 10 artists, 8 shows, 6 radios) plus an 800-entry track LRU, pruned on the same tick | `S/src/app.rs:6350-6356` |
| Image disk | `<cache>/art/<sha1(url)>`, `.part` then rename, no cap, manual clear only | `S/src/images.rs:185`, `:225-281` |
| Placeholders | Previous image, then a softened thumbnail, then a flat fill. No spinners | `S/src/ui/widgets.rs:80-131` |
| Virtualisation | `virtual_rows` lays out only rows inside the clip rect, fixed row heights (56/48/36), spacers for the rest. Tables size to the playlist's total and load 50-item windows by offset as they scroll into view | `S/src/ui/widgets.rs:213-265`, `S/src/ui/collection.rs:575-578`, `:809`, `S/src/backend.rs:49` |
| Prefetch | Almost none: no hover, viewport or page prefetch; hover only extracts a tint | `S/src/app.rs:8239`, `S/src/player.rs:912-913` |
| Build | `lto = "thin"`, `codegen-units = 1`, `strip = true`, `panic = "abort"`; every dependency at `opt-level = 2` in dev. glow instead of wgpu, trimmed `image` features, no custom allocator, no target-cpu, no mold | `S/Cargo.toml:32-35`, `:213-223` |

## Where FormalMusic already matches or beats it

- Paints Home, Explore, library, queue and player from `state.json` before the daemon
  answers, read on a thread beside window setup (`crates/core/src/cache.rs`,
  `crates/desktop/src/main.rs:56-58`). Spotifast has no equivalent.
- Stale-while-revalidate pages and first-target prefetch (`crates/core/src/store.rs:34-41`, `:556-595`).
- Decode at display size with stepped CDN sizes and a 64 MiB byte LRU (`crates/desktop/src/art.rs:22`, `:198-223`,
  `crates/core/src/art.rs:17`). Spotifast relies on egui's loader and does not downscale.
- Device suspended on pause, and an engine that sleeps until a command when paused
  (`crates/player/src/output.rs:222-228`, `crates/player/src/engine.rs:226`).
- Gapless preload plus two queue entries resolved ahead (`crates/daemon/src/playback.rs:33`, `:541-601`).
- Narrow per-topic repaints through `bridge.rs`, which is better than egui's whole-frame model.

## What to adopt, by expected impact

| # | What to do | Where in FormalMusic | From Spotifast |
|---|---|---|---|
| 1 | Bound the in-memory page map. Keep the visible route and history entries, cap the rest per kind (playlists, albums, artists), and drop the oldest when a page lands. Without this, the 1000-track playlist and every page visited stay resident and the 120 MB RSS budget fails over a long session | `crates/core/src/store.rs:99` (`pages: HashMap`, no eviction anywhere) | `S/src/app.rs:6350-6356`, `:2831-2834` |
| 2 | Cap art concurrency. One `Semaphore` (about 6) around `ArtCache::fetch` and a second (2, or cores minus 2) around decode. Today a cold Home spawns one download and one `spawn_blocking` decode per cover at once, which spikes CPU and peak RSS and lets off-screen covers race visible ones | `crates/desktop/src/art.rs:136-139`, `crates/core/src/art.rs:39-58` | `S/src/api/client.rs:23`, `S/src/backend.rs:1404`, `:915-916` |
| 3 | Release profile: `codegen-units = 1`, `strip = true`. Add `[profile.dev.package."*"] opt-level = 2` so symphonia decode and layout are realistic in dev builds and timings from `FORMALMUSIC_TRACE` mean something | `Cargo.toml:29-31` | `S/Cargo.toml:213-223` |
| 4 | Turn on reqwest's `http2` feature. InnerTube calls and lh3.googleusercontent art then multiplex over one connection per host instead of opening up to six HTTP/1.1 connections on a cold Home. Keep the audio client (`crates/player/src/fetch.rs:37`) as is; range reads gain nothing | `Cargo.toml:23` | `S/Cargo.toml:133`, `S/src/http.rs:12-71` |
| 5 | Interpolate position in the client. Store `position_at: Instant` with each `Position` event and compute the shown position at render; the daemon then emits on play, pause, seek and once a second instead of four times. Fewer JSON lines parsed and fewer store writes, and the seek bar can move smoothly while visible | `crates/core/src/store.rs:449-459`, `crates/daemon/src/playback.rs:724-736`, `crates/player/src/engine.rs:20` | `S/src/player.rs:216-226`, `:344` |
| 6 | Per-playlist disk cache for long playlists, written in chunks as continuations arrive and read with `spawn_blocking` on open, so a 1000-track playlist paints in the first frame after a restart, not only Home and the library tabs. Revalidate by `fetched_at` since InnerTube has no snapshot id | `crates/core/src/cache.rs` (only Home, Explore, library today) | `S/src/backend.rs:4018-4030`, `:3168-3200`, `:4252-4310` |
| 7 | Reject art payloads over 8 MiB before decode, so one bad URL cannot allocate a huge buffer | `crates/core/src/art.rs:68-77` | `S/src/images.rs:21`, `:270` |
| 8 | Keep a lower-resolution entry on screen while a larger size decodes (for example the 60 px row cover under the 544 px header) instead of the flat `palette.raised` fill | `crates/desktop/src/art.rs:90-97` | `S/src/ui/widgets.rs:80-110` |

Verify each with `FORMALMUSIC_TRACE=1` against the PLAN.md budget table: cold start
to cached Home, RSS after Home, artist, 1000-track playlist and player, and CPU while
paused and unfocused.

## What not to copy

| Spotifast does | Why not here |
|---|---|
| `panic = "abort"` (`S/Cargo.toml:217`) | The profile is workspace-wide, so `formalmusicd` would get it too. A panic in one tokio task (a bad parse, a yt-dlp edge case) would kill playback and MPRIS instead of failing one request. Spotifast had to replace librespot's sink to survive it (`S/Cargo.toml:124-128`) |
| Unbounded art disk cache (`S/src/images.rs:185`) | Same gap exists in `crates/core/src/art.rs`. Add a size cap pruned by mtime off-thread at startup rather than copying the manual-clear approach |
| Timer polling while connected, 4 s or 20 s (`S/src/app.rs:9710-9711`, `:9923-9928`) | egui is immediate mode and needs it. GPUI plus the `subscribe` event stream is push-driven, so any repaint timer here is a regression against the 0% idle target |
| Offset windows sized to the playlist total (`S/src/ui/collection.rs:575-578`, `:809`) | InnerTube continuations are sequential tokens with no random offset access. The current append-and-splice in `crates/desktop/src/page.rs:188-248` is the right shape |
| Asking the CDN for the smallest variant at or above 64 px and letting the GPU scale (`S/src/api/models.rs:123-146`) | FormalMusic's stepped sizes and decode-at-box path is already better and avoids aliasing on GPUI's unmipmapped textures |
| A patched egui fork (`S/Cargo.toml:233-256`) | Pins every UI crate to a private revision. Upstream fixes to gpui-kit instead |
| glow over wgpu (`S/Cargo.toml:32-35`) | Does not apply: GPUI has its own renderer |
| A custom polyphase resampler (`S/src/resample.rs`) | Spotify output is fixed at 44.1 kHz, YouTube opus is 48 kHz. Only worth it if a rate mismatch actually crackles in our engine |
