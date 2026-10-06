# Synced lyrics view

Spec for rebuilding the synced-lyrics view in FormalMusic's GPUI desktop app
(`crates/desktop/src/now_playing.rs`, `LyricsView` at line 355) so it matches
the two reference implementations the owner built: FormalShell's media panel
lyrics pane (QML) and Kopuz's `LyricsView` (Dioxus, Rust plus injected JS).

## Sources and abbreviations

| Tag | File |
| --- | --- |
| FS-P | `FormalShell/shell/Surfaces/Panels/LyricsPane.qml` |
| FS-M | `FormalShell/shell/Lyrics/model.js` |
| FS-S | `FormalShell/shell/Services/LyricsService.qml` |
| FS-MP | `FormalShell/shell/Surfaces/Panels/MediaPanel.qml` |
| FS-T | `FormalShell/shell/Theme/tokens.js` (font and motion tokens) |
| KZ-L | `kopuz/crates/components/src/playback/lyrics.rs` |
| KZ-CB | `kopuz/crates/components/src/playback/cover_background.rs` |
| KZ-C | `kopuz/crates/utils/src/color.rs` |
| KZ-PC | `kopuz/crates/hooks/src/use_player_controller.rs` |
| GP | `~/.cargo/registry/src/index.crates.io-*/gpui-pre-0.3.6/src/` (what `gpui_kit::*` re-exports, gpui-kit `lib.rs:95`) |
| GB | `.../gpui-base-0.6.6/src/` (`gpui_kit::base`) |

Kopuz reference clone: `~/Developer/kopuz-ref` (depth 1, 2026-10-06).

### Views inventory

* FormalShell has exactly one lyrics view: the pane on the right of the media
  panel (FS-MP:860, FS-P whole file). There is no bar ticker or lock-screen
  lyrics; `rg -il lyric shell/` finds only the service, the model, the pane,
  the panel, config docs and IPC.
* Kopuz has one component, `LyricsView` (KZ-L:401), mounted in two layouts:
  `Fullscreen` (desktop tab `fullscreen/tabs.rs:272`, Android
  `fullscreen/android.rs:122`) and `Rightbar` (`layout/rightbar.rs:151`). The
  layouts differ only in type size, blur ramp and widths.
* FormalMusic's target is the 440px Lyrics tab of the expanded player
  (`now_playing.rs:28`, `:220`), which sits between Kopuz's Rightbar and
  Fullscreen in size. Values below give both sources, then the FormalMusic
  target.

### Which source wins

FormalShell is a later port of Kopuz with owner corrections dated 2026-09-18
in its comments (FS-P:26, :37, :54, :286). Where the two disagree, the target
follows FormalShell's behaviour and borrows Kopuz values only for things
FormalShell does not have (backdrop, larger type).

## 1. Data, timing and the lit set

### Line model

FormalMusic already carries everything both sources need
(`crates/api/src/model.rs:278-318`): `LyricLine { start_ms, end_ms, text,
words: Vec<LyricWord>, background, agent, opposite_turn }` and `LyricWord {
start_ms, end_ms, text, joins_next }`. Add a display-only `interlude: bool`
and `estimated: bool` (synthesised words) when building display lines; do not
put them on the wire type.

Both sources ignore a word's own end stamp: a chunk ends where the next chunk
starts, else at the line's end, else 0.35 s after its own start (KZ-L:295-303,
FS-M:646-653). Keep that for parity; `LyricWord::end_ms` stays unused by the
view.

### Lit set

Port Kopuz's functions directly (they are already Rust): `main_line_indices`
(KZ-L:175), `next_main_line_start` (:188), `line_active_at` (:200),
`active_main_line_index` (:226), `background_line_bound` (:250),
`active_secondary_lines` (:264), `line_end_estimate` (:320),
`build_display_lines` (:334). FormalShell's JS ports are FS-M:895-1094. These
replace `formalmusic_core::store::line_at` (`crates/core/src/store.rs:274`),
which only answers "last line started" and cannot light background or duet
lines.

| Rule | Value | Ref |
| --- | --- | --- |
| Main lines | every non-background line, or all lines if all are background | KZ-L:175, FS-M:895 |
| Line with no end | lit until the next main line starts | KZ-L:209, FS-M:927 |
| Carry across a short gap | lit past its end while the next main start is at most 3.0 s after the end | KZ-L:38, :219; FS-M:87, :933 |
| Background line | judged on its own timing; with no end, runs to the first main start after it | KZ-L:250; FS-M:964 |
| Opposite (duet) line | lit only while some main line is lit | KZ-L:283; FS-M:991 |
| Several lit at once | the active main line plus every secondary returned | FS-P:13-18 |

### Interlude rows

| | FormalShell | Kopuz |
| --- | --- | --- |
| Intro gap | first main line starts after 3.0 s | first main line starts at or after 5.0 s |
| Between lines | gap at least 5.0 s, or the run has end stamps and the gap is over 3.0 s | gap at least 5.0 s |
| Gap start | max `line_end_estimate` over the run (main line plus its background lines), clamped to `[current.start, next.start]` | same |
| `line_end_estimate` | `end`, else last real word + 0.35 s, else start + 7 s (ignores synthesised words) | `end`, else last chunk + 0.35 s, else start + 7 s |
| Refs | FS-M:1020-1094, :1005 | KZ-L:334-399, :320 |

Target: FormalShell's two rules (they close the 3 to 5 s hole where nothing was
lit, FS-M:1048-1055).

### Chunk progress and glow

```
chunk_end   = next_chunk.start ?? line_end ?? start + 0.35
span        = estimated ? (chunk_end - start) : min(chunk_end - start, 1.2)
fill        = clamp((t - start) / span, 0, 1)            // 1 if span <= 0 and t >= start
glow        = t < start ? 0 : t <= chunk_end ? 1 : 1 - (t - chunk_end) / 0.6
glow        = round(clamp(glow, 0, 1) / 0.05) * 0.05
```

Refs: KZ-L:486, :551-580; FS-M:68, :73-74, :669-693. `line_end` for a display
row is its own `end`, else the next display row's start (FS-P:313-318).
Kopuz also quantises `fill` to 1/200 (KZ-L:559); that only exists to limit DOM
writes and is not needed in GPUI.

Interlude progress: `clamp((t - start) / (end - start), 0, 1)`, only while the
interlude row is lit; a dark interlude resets to 0 (FS-P:326-331, KZ-L:524-541,
:621-625).

### Synthesised word timing (line-only synced lyrics)

| | FormalShell | Kopuz |
| --- | --- | --- |
| Behaviour | splits the line on whitespace and spreads words over the line's span by character count | none: a line without chunks lights as a whole (KZ-L:1056) |
| Span | `end`, else next main start, else start + 7 s | n/a |
| Word time | `start + span * chars_before / total_chars` | n/a |
| Wipe cap | off (`estimated = true` disables the 1.2 s cap) | n/a |
| Skips | interludes, lines containing `\n` (merged translations) | n/a |
| Ref | FS-M:714-761, :661-668 | KZ-L:1056-1058 |

Target: FormalShell. Run it over the display lines, never the raw lines
(FS-M:699-710).

### Playback clock

| | FormalShell | Kopuz |
| --- | --- | --- |
| Source | MPRIS position refreshed every frame by a `FrameAnimation` while the panel is open, playing and synced (FS-MP:158-161) | engine anchor `(ms, Instant)` plus elapsed while playing (KZ-PC:408-424); JS extrapolates between pushes, capped at 0.1 s (KZ-L:492-496) |
| Lead | +0.1 s (`POSITION_LEAD_SECONDS`, FS-M:82) | none |
| Hold | output latency from `pw-dump` when `media.lyricsOffsetAuto` (default on), plus manual `media.lyricsOffsetMs` (-5000..5000) (FS-M:768-782, FS-S:63-82) | output latency when auto (default), else manual offset (-1000..1000) (KZ-L:883-896) |
| Formula | `pos + 0.1 - hold` | `pos - hold` |
| Push rate | per frame | Rust loop sleeps `clamp(next_start - t, 16, 50)` ms (KZ-L:916-922), JS paints per rAF |

Target: FormalMusic's store moves `position_ms` four times a second
(`crates/core/src/store.rs:100`), so the view must extrapolate. Keep an anchor
`(position_ms, Instant)` updated on every `StoreEvent::Position`; while
playing, `t = anchor_ms/1000 + min(elapsed, 0.5 s) + 0.1 - offset_ms/1000`.
The cap is 0.5 s rather than Kopuz's 0.1 s because the feed interval is
250 ms. Expose `lyrics_offset_ms` (-5000..5000, default 0) in config; output
latency auto-detection is out of scope until the daemon reports it.

## 2. Layout

| Property | FormalShell | Kopuz Fullscreen | Kopuz Rightbar | Target |
| --- | --- | --- | --- | --- |
| Pane | half of an 840px panel, minus `sm` gap (FS-MP:446, FS-T:102) | 50% of window (`desktop.rs:157`) | right bar | 440px tab |
| Container padding | Cell insets `controlPaddingX` 12px each side (FS-P:381-385, FS-T:96) | `px-4 py-2` + column `px-8 py-4` (KZ-L:956, :966) | `px-2 py-2` + `px-4 py-4` (KZ-L:957, :968) | 12px x |
| Column max width | pane width | `max-w-2xl` 672px (KZ-L:966) | pane | pane |
| Line width, no duet | 100% | `min(100%, 38rem)` | `min(100%, 20rem)` | 100% |
| Line width, duet track | 90% for every line (FS-P:386) | `min(90%, 34rem)` | `min(90%, 18rem)` (KZ-L:137-148) | 90% |
| Alignment, no duet | left (FS-P:724) | centre (KZ-L:15-18) | centre | left |
| Alignment, duet | main left, opposite right (FS-P:387, :485-488) | same (KZ-L:164-168) | same | same |
| Line gap | 0, rows are `Cell`s with their own padding (FS-P:239) | `gap-4` 16px + `space-y-1` (KZ-L:956, :966) | same | Cell-style row padding, 0 gap |
| Active line anchor | row top at 42% of viewport height (FS-M:96, :1228) | row top at 42% via 42% top spacer and 58% tail spacer (KZ-L:36-37, :654-661, :961, :1084) | same | 42% |

The pane is a clipped viewport with a translated column, not a scroll view
(FS-P:185-192, :236-262). Rows never change size on activation: activation is a
transform, never a relayout (FS-P:65-66, :401-402), and every row lays out its
chunks whether lit or not so wrapping never changes (FS-P:20-28).

## 3. Typography

| Row | FormalShell | Kopuz Fullscreen | Kopuz Rightbar | Target |
| --- | --- | --- | --- | --- |
| Family | `Theme.fontFamilySans` (FS-P:198) | inherited body: JetBrains Mono first (`crates/kopuz/assets/main.css:139-142`) | same | `theme::font_sans()` |
| Main size / line height | `title` = 15px at base 13 (FS-T:16, :25-29) / font default | `text-2xl` 24px / 32px | `text-lg` 18px / 28px | 22px / 30px (`type_scale::LYRIC`, `theme.rs:86`) |
| Main weight | 500 medium (FS-P:200, FS-T:149) | 600 semibold (KZ-L:5) | 600 | 600 |
| Background size | `body` 13px (FS-P:304-305) | `text-xl` 20px, `leading-snug` 1.375 (KZ-L:20) | `text-sm` 14px, snug (KZ-L:22) | 18px / 24px |
| Background weight | 500 | 500 | 500 | 500 |
| Opposite line | italic (FS-P:551, :729) | italic (KZ-L:32-35) | italic | italic |
| Letter spacing | none | none | none | none |
| Plain fallback | not shown | `text-lg` 18px, 500, `leading-relaxed` 1.625, white/70, centred, `pre-wrap` (KZ-L:966, :1076) | `text-sm` | see section 9 |

Word spacing: FormalShell lays words out in a `Flow` spaced by one space
advance of the row's font (FS-P:194-210, :489); chunks inside a word sit with
zero spacing (FS-P:494-497).

## 4. Line states

There is no separate "past" state in either source: past and upcoming lines
differ only by their distance from the anchor.

| Property | FormalShell | Kopuz |
| --- | --- | --- |
| Lit ink | `foreground` (sung) over `mutedForeground` (unsung) (FS-P:552, :602) | white; unsung part white at 0.45 (KZ-L:481, :508-510) |
| Unlit ink | `mutedForeground` (FS-P:552, :730) | `text-white/40`, hover `text-white/60` (KZ-L:5) |
| Lit scale | 1.0; background or interlude 0.9 (FS-P:408-410) | 1.12 single voice, 1.06 duet track or interlude, 1.02 background (KZ-L:111-122, :993) |
| Unlit scale | 0.85 (FS-P:410) | 1.0 |
| Transform origin | left; right for opposite; centre for interlude on a non-duet track (FS-P:413-415) | centre; left on duet track; right for opposite (KZ-L:124-135) |
| Lit opacity | 1 | 1 |
| Unlit opacity | depth ramp `[1, 0.7, 0.45, 0.25]` sampled over the rows that fit (see below) (FS-M:104, :1104-1115) | 1 (only blur varies) |
| Background line opacity | x 0.7 on top of the ramp (FS-P:400) | ink white/25 unlit, white/70 lit (KZ-L:20-23); wipe alpha 0.7 (KZ-L:498) |
| Arrival fade | opacity 0.68 to 1, `effectsSlow` 300ms, bezier (0.34, 0.88, 0.34, 1) (FS-P:365-377, FS-T:235, :265) | 0.68 to 1, 260ms `ease-out` (KZ-L:704-710) |
| Depth blur | `min(|d| / span, 1) * 6px * strength%`, quantised to 0.5px (FS-M:91-93, :1165-1171) | `min(|d| * step, max) * strength%`, quantised 0.5px; Fullscreen step 1.5 max 8, Rightbar step 1.1 max 6 (KZ-L:51-54, :715-757) |
| Blur exempt | lit rows, hovered row, keyboard-cursor row (FS-P:351-354) | lit rows, rows more than one viewport height from the anchor (KZ-L:749-755) |
| Blur strength | `media.lyricsBlurStrength` 0..200, default 100; `media.lyricsBlur` on/off (FS-S:63-64) | 10..200 default 100; on/off (KZ-L:744; `config/src/lib.rs:73`) |
| Transitions | opacity `effects` 200ms (0.34, 0.8, 0.34, 1); scale `spatialFast` 350ms (0.42, 1.67, 0.21, 0.9) (overshoots); blur `effects`; colour 300ms `effectsSlow` (FS-P:357-359, :417-422, :732-734; FS-T:234-265) | color, transform, filter 300ms CSS `ease`; opacity 180ms (KZ-L:19) |

Depth ramp details (FormalShell, target):

```
anchor     = active main index, else highest lit secondary, else last anchor   (FS-P:105-113)
row_pitch  = column_height / row_count                                          (FS-P:154-156)
span_above = max(1, 0.42 * viewport_h / row_pitch)                              (FS-M:1123-1131)
span_below = max(1, 0.58 * viewport_h / row_pitch)
span       = d < 0 ? span_above : span_below
at         = min(3, |d| * 3 / span)
opacity    = lerp(TABLE[floor(at)], TABLE[floor(at)+1], fract(at))              TABLE = [1, 0.7, 0.45, 0.25]
blur_px    = round(min(|d| / span, 1) * 6 * strength/100 / 0.5) * 0.5
```

Edge fade (FormalShell only, FS-M:1149-1156, FS-P:164-165, :288, :338-339),
multiplied into the row opacity every frame, not animated:

```
ramp      = max(32, space_advance_row_height + 2 * 6)
clearance = min(row_top, viewport_h - (row_top + row_h))     // row_top after the column's travel
edge      = clamp(clearance / ramp, 0, 1)
row_opacity = edge * arrival * (lit ? 1 : depth_opacity) * (background ? 0.7 : 1)
```

## 5. Word and syllable highlighting

Both sources use the same gradient wipe. Kopuz paints it with
`background-clip: text` (KZ-L:504-520, :551-563); FormalShell reproduces it
with a mask over a second copy (FS-P:589-703).

| Property | Value | Ref |
| --- | --- | --- |
| Gradient | horizontal, 2.2 chunk widths, sung 0% to 46%, ramp 46% to 54%, unsung 54% to 100% | KZ-L:510, FS-P:657-666 |
| Position | `x = -1.2 * w * (0.99 - 0.98 * fill)` | KZ-L:562 (`background-position-x (99 - 98 fill)%` with 220% size), FS-P:659 |
| Unsung | Kopuz: sung alpha x 0.45. FormalShell: the `mutedForeground` copy underneath (its mask thresholds instead of multiplying) | KZ-L:481, FS-P:40-43, :606-621 |
| Background line alpha | sung and unsung x 0.7 | KZ-L:498 |
| Glow | blur `4 + 6 * glow` px, colour foreground at `0.3 * glow * line_alpha`, no offset | KZ-L:577-579, FS-P:696-701 |
| Wrapped chunk | one band per text row, wiped in reading order, travel weighted by each row's ink width | FS-M:1173-1219, FS-P:629-669 |
| Lift, per-word scale, long-word emphasis, per-letter animation | not present in either source | KZ-L:543-584, FS-P:502-704 |
| Reduced motion | fill snaps 0 or 1, glow off | KZ-L:487, :557, :566 |

The position formula reduces to a clean edge in chunk-local coordinates
(`w` = chunk ink width, `f` = fill):

```
edge_hi = 1.176 * w * f         // right of this: fully unsung
edge_lo = edge_hi - 0.176 * w   // left of this: fully sung; linear between
```

At `f = 0` the ramp ends exactly at the chunk's left edge, at `f = 1` it starts
exactly at its right edge. The soft band is 17.6% of the chunk width.

Only lit rows run the wipe; a row going dark resets its chunks to unsung
(KZ-L:605-626, FS-P:514-524, :589-591).

## 6. Background vocals and duet

| | FormalShell | Kopuz | Target |
| --- | --- | --- | --- |
| Background size | `body` (smaller step) | `text-xl` / `text-sm` | 18px |
| Background indent | one `controlPaddingX` (12px) on its voice's side (FS-P:424-429) | `pl-6` 24px (Fullscreen), `pl-4` 16px (Rightbar), `pr-*` on opposite side; only on duet tracks (KZ-L:20-31) | 12px |
| Background lit | scale 0.9, opacity x 0.7 | white/70, scale 1.02 | FormalShell |
| Duet trigger | any line with `opposite_turn` makes the whole track two-sided (FS-P:115-123) | same (KZ-L:973) | same |
| Opposite line | right aligned, italic, origin right, 90% width | same, scale 1.06 | FormalShell |
| `agent` field | unused | unused (paxsenix `oppositeTurn` only) | unused |

## 7. Interlude indicator

Neither source draws three breathing dots. Both draw a Lucide `music` note
that fills left to right over the gap.

| | FormalShell | Kopuz Fullscreen / Rightbar | Target |
| --- | --- | --- | --- |
| Glyph | `music` icon (FS-P:448-468) | inline SVG of Lucide `music` (KZ-L:1005-1033) | `IconName::Music` (`icons.rs:62`) |
| Size | `heading` 17px (FS-T:16) | 28px / 20px (KZ-L:974-977) | 24px |
| Base copy | `mutedForeground` at 0.35 | white at 0.35 | `secondary` at 0.35 |
| Fill copy | `foreground`, clipped to `progress * width` from the left | white, `clip-path: inset(0 (100 - 100p)% 0 0)` | `text`, clipped |
| Row | centred, or leading edge on a duet track (FS-P:444) | `justify-center` / `justify-start` on duet, `py-2`, row opacity 0.4 unlit (hover 0.8), 1 lit (KZ-L:45-47, :305-318) | FormalShell |
| Scale | lit 0.9, unlit 0.85 (FS-P:403-410) | lit 1.06 | FormalShell |
| Click | seeks to gap start | same (KZ-L:998-1003) | same |

## 8. Scroll and follow

| | FormalShell | Kopuz |
| --- | --- | --- |
| Mechanism | column `y = 0.42 * viewport_h - row.y` (FS-M:1228-1230, FS-P:252-257) | `scrollTop` tween to the same target, clamped to the scroll range (KZ-L:654-691) |
| Curve | `Anim` default kind `spatial`: 500ms, bezier (0.38, 1.21, 0.22, 1), overshoots slightly (FS-P:259-262, FS-T:234, :261) | 720ms, `1 - (1 - t)^3` (KZ-L:676-678) |
| Retarget | Qt `Behavior` restarts from the current value | cancels the running tween, starts from current `scrollTop` (KZ-L:670-674) |
| Stagger per line | none | none |
| Drift fix | re-evaluates on column height change (FS-P:246-255) | re-tweens when off target by more than 24px and idle (KZ-L:695-702) |
| User takeover | wheel or trackpad over the pane sets `follow = false`; the wheel then moves `wheel_y` itself, clamped between the first and last row's resting positions; delta = pixel delta, else `angle_delta / 120 * 32px` (FS-P:216-234) | wheel, touchmove, arrow/Page/Home/End keys, pointer down on the scrollbar gutter (KZ-L:420-455) |
| Resume | resync button (`refresh-cw`, bottom right, fades in with `effects`), a new track, keyboard cursor entering the section, panel reopen (FS-P:758-775, FS-S:244-246, FS-MP:378-384, :424-432) | sync button (36px round, `bg-black/40`, bottom-right 16px), a new track (KZ-L:864-871, :1088-1106) |
| Timed resume | none | none |

Target: FormalShell's mechanism and curve. Resume triggers in FormalMusic: the
resync button, a new track, switching to the Lyrics tab, reopening the
expanded player.

## 9. Click, hover, keyboard, fallbacks

* Click a row: seek to `row.start` (FS-P:277, FS-MP:221-225; KZ-L:1050-1055).
  No per-word seek in either.
* Hover: FormalShell rows are ghost `Cell`s with the shell's hover wash and the
  hovered row drops its blur to 0 (FS-P:268-277, :351-354). Kopuz raises ink
  to white/60 (KZ-L:5). Target: `palette.hover_wash` row background (as
  today, `now_playing.rs:429`) plus blur 0 on hover.
* Keyboard: FormalShell's panel cursor walks the rows plus the resync button;
  the cursor row becomes the depth anchor (FS-P:113, FS-MP:378-403).
* Plain (unsynced) lyrics: FormalShell never shows them; the pane only exists
  when `state === "synced"` (FS-MP:216-217, FS-M:30). Kopuz renders one
  `pre-wrap` block at white/70 in the column (KZ-L:1075-1077). Target: keep
  FormalMusic's existing plain path (all lines in `text` at the main type,
  no state, no seek, `now_playing.rs:429-435`), rendered as a static column.
* No lyrics: FormalMusic's existing placeholders ("No lyrics for this song.",
  "Loading lyrics...") stay (`now_playing.rs:395-398`).

## 10. Colours and backdrop

| | FormalShell | Kopuz | Target |
| --- | --- | --- | --- |
| Lyric ink | theme tokens `foreground`, `mutedForeground` (live palette) | fixed white with alpha | `palette.text` sung/lit, `palette.secondary` unsung/unlit |
| Album-art colours | none in the pane | `color_thief::get_palette(pixels, Rgb, 10, 8)` on a 400px thumbnail (KZ-C:38-60); each colour dimmed to luminance at most 90 (KZ-C:69-81) | existing 24px backdrop decode |
| Gradient backdrop | none (pane is a `Card`) | base colour 0 plus up to 7 radial gradients at 0% 0%, 100% 0%, 100% 100%, 0% 100%, 50% 50%, 25% 0%, 75% 100%, alpha 0.8 fading to transparent at 80% (KZ-C:83-120) | not needed |
| Cover backdrop | none | cover `object-cover`, `blur(b px)` with `scale(1 + 0.004 b)`, `b` default 0, max 100; black scrim at 60% default, max 95 (KZ-CB:8-40) | already present: cover decoded at 24px and stretched (`art.rs:24-27`, `now_playing.rs:195-196`) under `canvas` at 0xc8 |
| Animated backdrop | none | none | none |

## 11. GPUI mapping

### What GPUI offers (gpui-pre 0.3.6)

| Need | API | Ref |
| --- | --- | --- |
| Shape a run | `window.text_system().shape_line(text, font_size, &[TextRun], None) -> ShapedLine` | GP `text_system.rs:638` |
| Wrapped text | `shape_text(..., wrap_width, line_clamp) -> SmallVec<[WrappedLine; 1]>` | GP `text_system.rs:750` |
| Glyph positions | `ShapedLine` derefs to `LineLayout { width, ascent, descent, runs: Vec<ShapedRun { font_id, glyphs: Vec<ShapedGlyph { id, position, index, is_emoji }> }> }` (all public) | GP `text_system/line_layout.rs:19-56` |
| Paint one glyph | `window.paint_glyph(origin, font_id, glyph_id, font_size, color: Hsla)`; origin y is the baseline | GP `window.rs:4647` |
| Per-run colour | `TextRun { len, font, color, background_color, underline, strikethrough }`, `StyledText::with_runs` / `with_highlights` | GP `text_system.rs:1228`, `elements/text.rs:433`, `:526` |
| Clip | `window.with_content_mask(Some(ContentMask { bounds }), f)`, rectangles only, intersects with the parent | GP `window.rs:3989`, `:2127` |
| Quads, gradients | `paint_quad(fill(bounds, bg))`, `linear_gradient(angle, stop, stop)` (two stops only) | GP `window.rs:4510`, `color.rs:865` |
| Shadows | `BoxShadow { color, offset, blur_radius, spread_radius, inset }` on boxes only | GP `style.rs:349` |
| Opacity | `Styled::opacity(f32)` on an element and its children; `paint_glyph` multiplies by it | GP `styled.rs:746` |
| Custom element | `canvas(prepaint, paint)` | GP `elements/canvas.rs:10` |
| Per-frame | `window.request_animation_frame()` | GP `window.rs:2630` |
| Springs | `gpui_kit::base::spring(id, target, Spring::new(response).with_damping(r), window, cx)` keeps velocity across retargets | GB `motion.rs:573`, `:415-541` |
| Tweens | `with_animation(id, Animation::new(d).with_easing(f), ...)`; FormalMusic's `motion::cubic_bezier` for CSS/QML curves | GP `elements/animation.rs:15`, `:82`; `crates/desktop/src/motion.rs:35` |
| Wheel | `on_scroll_wheel(ScrollWheelEvent { delta: ScrollDelta::{Pixels, Lines}, touch_phase, .. })` | GP `elements/div.rs:1054`, `interactive.rs:522` |
| Reduced motion | `cx.reduce_motion()` | GP `app.rs:1138` |

Not available: gradient or mask fill on text, text shadow, any blur filter on
elements, text or images, element transforms (scale exists only for SVG via
`Transformation`, GP `elements/svg.rs:214`), letter spacing. Glyph raster
positions snap to whole device pixels vertically (`SUBPIXEL_VARIANTS_Y = 1`,
GP `text_system.rs:52`) and to quarter pixels horizontally.

### Architecture

Replace the `list(...)` in `LyricsView` with one custom-painted column:

1. **Layout once per (track, width)**, cached on the view. For every display
   row, shape each chunk with `shape_line` at the row's full (lit) font size,
   group chunks into words by `joins_next`, and wrap words greedily into text
   rows at the row's content width, spacing words by the shaped width of
   `" "`. This is FormalShell's `Flow` of `Row`s (FS-P:477-500) and gives each
   chunk its rectangle and glyph list. A single chunk wider than the row
   falls back to `shape_text` with wrapping and gets per-text-row bands
   (FS-M:1173-1219). Rows without words lay out the same way from their text.
   Row height comes from this layout and never changes with state.
2. **Paint per frame** in a `canvas` sized to the viewport, clipped
   (`overflow_hidden` on the wrapper). For each row whose rect intersects the
   viewport: compute its state (lit, distance, depth opacity, blur px, scale,
   edge fade, arrival fade), then paint its glyphs with `paint_glyph` using
   the transforms below.
3. **Clock and frames**: while the Lyrics tab is visible, the track is
   playing and lyrics are synced, call `window.request_animation_frame()` from
   render. Otherwise render only on store events. Drop the `Topic::LyricLine`
   subscription; the view computes the lit set itself from the extrapolated
   clock (section 1).
4. **Hit testing**: one absolutely positioned transparent `div` per visible
   row (or one `on_mouse_down` on the canvas wrapper mapping y to row) for
   click-to-seek, hover and cursor style.

### Effect by effect

| Effect | GPUI approach | Fidelity |
| --- | --- | --- |
| Row scale 0.85 / 0.9 / 1.0 about left, right or centre | Paint glyphs at `font_size * s` and position `origin + (glyph_pos - origin) * s`, origin = row's transform origin. Layout stays at s = 1. Quantise animated `s` so `font_size * s` lands on 0.25px steps to bound glyph-atlas entries during the 350ms animation | Exact geometry. Hinting differs slightly per size; text may shimmer a little mid-animation |
| Scale curve (0.42, 1.67, 0.21, 0.9), 350ms, overshoot | Per-row `(from, to, started)` stepped with `motion::cubic_bezier`; restart from current value on change | Exact |
| Opacity ramp, background 0.7, arrival fade, edge fade | Multiply into each glyph's `Hsla.a`; arrival and ramp changes tweened with `cubic_bezier` (0.34, 0.8, 0.34, 1) 200ms and (0.34, 0.88, 0.34, 1) 300ms. The arrival starts from the opacity the row shows when it lights, not from 0.68: both references restart at 0.68 and so dim a near row for one frame before fading it up, which reads as the line darkening as it lights. The word glow rises over 200ms instead of switching on, and blur copies share their alpha out only above 1px, so a row easing out of blur never steps | Deliberate deviation |
| Unlit/lit ink colour crossfade 300ms | Lerp `secondary` to `text` per glyph | Exact |
| Word wipe with soft edge | Three clip regions per chunk text-row, painted with the same glyphs: left of `edge_lo` in sung ink, right of `edge_hi` in unsung ink, and the band between split into 8 vertical strips, each clipped with `with_content_mask` and painted in `lerp(unsung, sung, strip_centre)`. Regions do not overlap, so alpha stays correct | Band quantised to 8 steps over 17.6% of the chunk (about 2 to 4px per step at 22px); invisible at normal reading distance |
| Wrapped chunk wipe | Same per band, using `row_wipe` progress (FS-M:1204-1219) | Exact |
| Glow on the sung chunk | Before the chunk's own glyphs, paint its glyphs 8 more times at offsets on a circle of radius `r = (4 + 6g)/2` px (plus 4 at `r/2`), colour `text` at per-tap alpha `a_tap = 1 - (1 - 0.3 g)^(1/12)` | Approximate Gaussian; slightly lumpy at large radius, matches at the 4 to 10px used here. Cost: 12 glyph quads per glyph, only on the 1 to 3 glowing chunks |
| Depth blur 0.5 to 6px | Same multi-tap technique on whole unlit rows: blur `b` means 8 taps at radius `0.6 b` plus 4 at `0.3 b` plus the centre, per-tap alpha chosen so the summed centre coverage equals the row's opacity. Quantise `b` to 0.5px. Skip rows outside the viewport and rows with `b = 0`. Animate `b` with `effects` 200ms | Approximation: a box-ish blur built from offset copies, not a true Gaussian; reads as defocus at these radii. Fallback if too heavy: drop blur and let the opacity ramp carry depth (owner's original no-blur rule, FS-P:48) |
| Interlude note fill | `svg()` of `IconName::Music` twice: base at 0.35, fill inside a `div().overflow_hidden().w(progress * size)` | Exact |
| Column follow (500ms, bezier (0.38, 1.21, 0.22, 1)) | Hold `column_y` on the view; on anchor change start a tween from the current value with `motion::cubic_bezier`; paint rows at `row.y + column_y` | Exact. `gpui_kit::base::spring` is the alternative if a velocity-preserving retarget is wanted, at the cost of a different feel |
| Wheel takeover | `on_scroll_wheel` on the wrapper: `Pixels(d)` uses `d.y`, `Lines(l)` uses `l.y * 32px`; set `follow = false`, write `wheel_y`, clamp to resting positions of the first and last rows | Exact |
| Resync button | `IconButton` bottom-right, fade in with `motion::fade` when `!follow` | Exact. Needs a Lucide `refresh-cw` glyph added to `IconName` |
| Backdrop | Keep the existing 24px stretched cover under the `canvas` wash | No change |
| Reduced motion | `cx.reduce_motion()`: fill 0 or 1, glow off, scale and scroll jump | Exact |

### Paint budget

At 22px in a 440px tab, a viewport shows about 12 rows of up to 40 glyphs.
Unblurred: about 500 glyph quads. With every unlit row blurred at 13 taps:
about 6,000 glyph quads per frame, all from the atlas. GPUI batches sprites per
atlas texture, so this is one or two draw calls; the cost to watch is CPU-side
scene building. Measure with the `frame-overlay` feature
(`crates/desktop/Cargo.toml:8`) before shipping blur; if a frame exceeds 4ms,
cap blurred rows to the 6 nearest the anchor.

## 12. Constants

```rust
// Timing (FS-M:60-96, KZ-L:36-54)
const POSITION_LEAD: f64 = 0.10;
const CLOCK_EXTRAPOLATION_CAP: f64 = 0.50;   // FormalMusic only, feed is 4 Hz
const CARRY_GAP: f64 = 3.0;
const INTERLUDE_MIN: f64 = 5.0;
const LINE_ASSUMED: f64 = 7.0;
const CHUNK_FALLBACK: f64 = 0.35;
const WIPE_MAX: f64 = 1.2;
const GLOW_DECAY: f64 = 0.6;
const GLOW_QUANTUM: f32 = 0.05;

// Layout
const COMFORT_OFFSET: f32 = 0.42;
const DUET_WIDTH: f32 = 0.90;
const ROW_PAD_X: Pixels = px(12.);
const BACKGROUND_INDENT: Pixels = px(12.);

// States
const SCALE_UNLIT: f32 = 0.85;
const SCALE_LIT: f32 = 1.0;
const SCALE_LIT_SECONDARY: f32 = 0.9;        // background lines, interludes
const DEPTH_OPACITY: [f32; 4] = [1.0, 0.7, 0.45, 0.25];
const BACKGROUND_ALPHA: f32 = 0.7;
const BLUR_MAX_PX: f32 = 6.0;
const BLUR_QUANTUM_PX: f32 = 0.5;
const INTERLUDE_BASE_ALPHA: f32 = 0.35;

// Wipe gradient
const WIPE_BAND: f32 = 0.176;                // of chunk width
const WIPE_TRAVEL: f32 = 1.176;
const GLOW_ALPHA: f32 = 0.3;
const GLOW_RADIUS: (f32, f32) = (4.0, 6.0);  // base + per unit of glow

// Motion (FS-T:234-265)
const SCROLL: (Duration, (f32, f32, f32, f32)) = (ms(500), (0.38, 1.21, 0.22, 1.0));
const SCALE_ANIM: (Duration, (f32, f32, f32, f32)) = (ms(350), (0.42, 1.67, 0.21, 0.9));
const FADE: (Duration, (f32, f32, f32, f32)) = (ms(200), (0.34, 0.8, 0.34, 1.0));
const ARRIVAL: (Duration, (f32, f32, f32, f32)) = (ms(300), (0.34, 0.88, 0.34, 1.0));
```
