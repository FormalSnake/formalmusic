//! The expanded player: the cover, a Song/Video switch when the track has both, and the Up
//! next, Lyrics and Related tabs. Lyrics are nd's `lyrics_view` on the same clock as the bar:
//! it follows the song, gives way to a scroll, and resyncs from its button or the next track.

use std::sync::Arc;
use std::time::Duration;

use formalmusic_api::{Item, PlaybackMode, SectionLayout};
use formalmusic_core::StoreEvent;
use nd::prelude::*;

use crate::art;
use crate::bridge;
use crate::items::{self, RowEnv};
use crate::menus::MenuContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    UpNext,
    Lyrics,
    Related,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::UpNext, Tab::Lyrics, Tab::Related];

    fn title(self) -> &'static str {
        match self {
            Tab::UpNext => "Up next",
            Tab::Lyrics => "Lyrics",
            Tab::Related => "Related",
        }
    }
}

pub struct NowPlayingView {
    pub tab: Tab,
    /// The root's presentation of this view: while it closes, the cover goes back to the bar.
    presentation: Presentation,
    /// The room the player was given: the artwork's size and whether it sits beside the panel.
    layout: PlayerLayoutState,
    queue: ListState<usize>,
    /// The video whose topics this view watches.
    watching: Option<String>,
    related: Option<String>,
    /// nd's lyrics for the playing video, converted once.
    lyrics: Option<(String, Arc<Lyrics>)>,
    shelves: Vec<ShelfHandle>,
}

impl NowPlayingView {
    pub fn new(tab: Tab, presentation: Presentation, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        for topic in [
            StoreEvent::NowPlaying,
            StoreEvent::Player,
            StoreEvent::Position,
            StoreEvent::Queue,
            StoreEvent::Ratings,
        ] {
            bridge::watch(cx, topic, id);
        }
        let queue = ListState::new(cx)
            .follow(Follow::None)
            .estimated_row_height(px(56.));
        Self {
            tab,
            presentation,
            layout: PlayerLayoutState::new(),
            queue,
            watching: None,
            related: None,
            lyrics: None,
            shelves: Vec::new(),
        }
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Follows the playing track: watches its lyrics and related page and asks for them.
    fn follow_track(&mut self, cx: &mut Context<Self>) {
        let store = crate::store(cx);
        let (video, related) = {
            let state = store.state();
            (
                state.current_video().map(str::to_owned),
                state.player.related_browse_id.clone(),
            )
        };
        let id = cx.entity_id();
        if video != self.watching {
            if let Some(old) = self.watching.take() {
                bridge::unwatch(cx, &StoreEvent::Lyrics(old), id);
            }
            if let Some(video) = &video {
                bridge::watch(cx, StoreEvent::Lyrics(video.clone()), id);
            }
            self.watching = video.clone();
        }
        if related != self.related {
            if let Some(old) = self.related.take() {
                bridge::unwatch(cx, &StoreEvent::Related(old), id);
            }
            if let Some(related) = &related {
                bridge::watch(cx, StoreEvent::Related(related.clone()), id);
            }
            self.related = related.clone();
        }
        match (self.tab, &video, &related) {
            (Tab::Lyrics, Some(video), _) => store.load_lyrics(video.clone()),
            (Tab::Related, _, Some(related)) => store.load_related(related.clone()),
            _ => {}
        }
    }

    fn queue_row(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let store = crate::store(cx);
        let state = store.state();
        let Some(track) = state.queue.tracks.get(index).cloned() else {
            return div().into_any_element();
        };
        let current = state.queue.current == Some(index);
        drop(state);
        let env = RowEnv {
            playing: current.then(|| (track.video_id.clone(), true)),
            menu: MenuContext {
                editable_playlist: None,
                queue_index: Some(index),
            },
            hide_artist: None,
            wide: false,
        };
        items::track_row(format!("queue-{index}"), &track, &env, None, cx)
    }

    fn lyrics(&mut self, video: &str, cx: &App) -> Option<Arc<Lyrics>> {
        if let Some((id, lyrics)) = &self.lyrics
            && id == video
        {
            return Some(lyrics.clone());
        }
        let store = crate::store(cx);
        let state = store.state();
        let source = state.lyrics.get(video)?.lyrics.clone()?;
        let lyrics = Arc::new(convert(&source));
        self.lyrics = Some((video.to_owned(), lyrics.clone()));
        Some(lyrics)
    }

    fn body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let store = crate::store(cx);
        match self.tab {
            Tab::UpNext => {
                let (len, digest) = {
                    let state = store.state();
                    let digest = state
                        .queue
                        .tracks
                        .iter()
                        .map(|t| fnv(&t.video_id))
                        .collect::<Vec<_>>();
                    (state.queue.tracks.len(), digest)
                };
                let current = store.state().queue.current;
                self.queue.set_items_versioned(
                    (0..len).map(|i| (i, digest[i] ^ u64::from(current == Some(i)))),
                    cx,
                );
                let queue = list("queue", &self.queue, cx.processor(Self::queue_row))
                    .on_select(cx.listener(|this, e: &SelectEvent<usize>, _, cx| {
                        this.queue.set_selected(Vec::new(), cx);
                        crate::store(cx).jump_to(e.key);
                    }))
                    .empty(
                        status_page("Nothing in the queue")
                            .icon(Icon::Music)
                            .description("Play something to fill it."),
                    );
                v_flex()
                    .size_full()
                    .gap_2()
                    .when(len > 0, |col| {
                        col.child(
                            h_flex()
                                .gap_1()
                                .justify_end()
                                .child(
                                    button("queue.save", "Save as playlist")
                                        .borderless()
                                        .on_click(|_, window, cx| crate::sheets::save_queue(window, cx)),
                                )
                                .child(
                                    button("queue.clear", "Clear")
                                        .borderless()
                                        .tooltip("Remove the songs after this one")
                                        .on_click(|_, _, cx| crate::store(cx).clear_queue()),
                                ),
                        )
                    })
                    .child(div().flex_1().min_h_0().w_full().child(queue))
                    .into_any_element()
            }
            Tab::Lyrics => {
                let (video, entry) = {
                    let state = store.state();
                    let video = state.current_video().map(str::to_owned);
                    let entry = video
                        .as_ref()
                        .and_then(|v| state.lyrics.get(v))
                        .map(|e| (e.loading, e.missing));
                    (video, entry)
                };
                let Some(video) = video else {
                    return div().into_any_element();
                };
                let lyrics = self.lyrics(&video, cx);
                let (clock, duration) = crate::player::clock(&store.state());
                let empty = match entry {
                    Some((true, _)) | None => "Loading lyrics\u{2026}",
                    _ => "No lyrics for this song.",
                };
                lyrics_view("lyrics", lyrics.unwrap_or_default(), clock)
                    .duration(duration)
                    .empty_text(empty)
                    .on_line_click(|e: &LyricClicked, _, cx| {
                        crate::store(cx).seek(e.start.as_millis() as u64)
                    })
                    .size_full()
                    .into_any_element()
            }
            Tab::Related => {
                let page = {
                    let state = store.state();
                    self.related
                        .as_ref()
                        .and_then(|id| state.related.get(id))
                        .and_then(|e| e.page.clone())
                };
                let Some(page) = page else {
                    return v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .child(spinner())
                        .into_any_element();
                };
                while self.shelves.len() < page.sections.len() {
                    self.shelves.push(ShelfHandle::new());
                }
                let env = RowEnv {
                    playing: None,
                    menu: MenuContext::default(),
                    hide_artist: None,
                    wide: false,
                };
                v_flex()
                    .id("related")
                    .size_full()
                    .overflow_y_scroll()
                    .gap_4()
                    .children(page.sections.iter().enumerate().map(|(i, section)| {
                        match section.layout {
                            SectionLayout::Carousel | SectionLayout::Grid => items::carousel(
                                format!("related-{i}"),
                                section,
                                &self.shelves[i],
                                cx,
                            ),
                            _ => v_flex()
                                .child(
                                    label(section.title.clone().unwrap_or_default())
                                        .text_style(TextStyle::Title3),
                                )
                                .children(section.items.iter().enumerate().map(|(j, item)| {
                                    match item {
                                        Item::Track(t) => items::track_row(
                                            format!("related-{i}-{j}"),
                                            t,
                                            &env,
                                            Some(std::rc::Rc::new({
                                                let item = item.clone();
                                                move |_, cx| {
                                                    crate::menus::play_item(
                                                        &item,
                                                        &crate::store(cx),
                                                        false,
                                                    )
                                                }
                                            })),
                                            cx,
                                        ),
                                        other => items::item_row(
                                            format!("related-{i}-{j}"),
                                            other,
                                            true,
                                            cx,
                                        ),
                                    }
                                }))
                                .into_any_element(),
                        }
                    }))
                    .into_any_element()
            }
        }
    }
}

impl NowPlayingView {
    /// The cover, moving when the album has motion artwork, or the music video in Video mode.
    fn picture(
        &self,
        track: &formalmusic_api::Track,
        mode: PlaybackMode,
        side: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = cx.entity_id();
        if mode == PlaybackMode::Video
            && let Some(frame) = crate::video::music_frame(id, cx)
        {
            return image(frame)
                .id("np.video")
                .alt(track.title.clone())
                .contain()
                .size(px(side))
                .rounded_lg()
                .into_any_element();
        }
        let cover = match crate::video::cover_frame(id, cx) {
            Some(frame) => image(frame).id("np.cover").alt(track.title.clone()),
            None => art::cover("np.cover", &track.thumbnails, side, track.title.clone()),
        };
        // The cover grows out of the bar's artwork as the player opens, and goes back into it.
        div()
            .size(px(side))
            .flex_none()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size(px(side))
                    .hero_from(BAR_ART, self.presentation.is_closing())
                    .child(cover.size_full().rounded_lg()),
            )
            .into_any_element()
    }
}

/// The player bar's artwork, which the cover travels from.
const BAR_ART: &str = "player.bar.art";

/// The daemon's lyrics as nd's: an empty line only marks where the line before it ends.
pub fn convert(source: &formalmusic_api::Lyrics) -> Lyrics {
    let ms = Duration::from_millis;
    let mut lines = Vec::with_capacity(source.lines.len());
    for (i, line) in source.lines.iter().enumerate() {
        if line.text.trim().is_empty() {
            continue;
        }
        let marker_end = source
            .lines
            .get(i + 1)
            .filter(|next| next.text.trim().is_empty())
            .map(|next| next.start_ms);
        let mut out = LyricLine::new(ms(line.start_ms), line.text.clone());
        out.end = line.end_ms.or(marker_end).map(ms);
        out.background = line.background;
        out.opposite = line.opposite_turn;
        out.words = line
            .words
            .iter()
            .map(|w| nd::components::media::LyricWord {
                start: ms(w.start_ms),
                text: w.text.clone().into(),
                joins_next: w.joins_next,
            })
            .collect();
        lines.push(out);
    }
    Lyrics {
        lines,
        synced: source.synced,
        source: source.source.clone().map(Into::into),
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

impl Render for NowPlayingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_track(cx);
        let store = crate::store(cx);
        let (track, mode) = {
            let state = store.state();
            (state.player.track.clone(), state.player.mode)
        };
        let Some(track) = track else {
            return status_page("Nothing playing")
                .icon(Icon::Music)
                .into_any_element();
        };
        let tab_index = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let body = self.body(cx);
        let wide = self.layout.is_wide();
        let picture = self.picture(&track, mode, self.layout.art_side(), cx);
        let details = v_flex()
            .gap_2()
            .when(wide, |d| d.items_center())
            .reveal_with(&self.presentation)
            .child(
                label(track.title.clone())
                    .text_style(if wide {
                        TextStyle::Title2
                    } else {
                        TextStyle::Headline
                    })
                    .truncate(),
            )
            .child(
                label(formalmusic_core::format::byline(&track))
                    .secondary()
                    .truncate(),
            )
            .when(track.counterpart.is_some(), |col| {
                col.child(
                    segmented("np.mode", ["Song", "Video"])
                        .selected(usize::from(mode == PlaybackMode::Video))
                        .on_change(|e: &SelectChanged, _, cx| {
                            crate::store(cx).set_mode(if e.index == 1 {
                                PlaybackMode::Video
                            } else {
                                PlaybackMode::Song
                            })
                        }),
                )
            });
        let panel = v_flex()
            .size_full()
            .gap_3()
            .reveal_with(&self.presentation)
            .child(
                segmented("np.tabs", Tab::ALL.map(Tab::title))
                    .selected(tab_index)
                    .on_change(cx.listener(|this, e: &SelectChanged, _, cx| {
                        this.set_tab(Tab::ALL[e.index.min(2)], cx)
                    })),
            )
            .child(div().flex_1().min_h_0().w_full().child(body));
        player_layout("now-playing", &self.layout)
            .artwork(move |_| picture)
            .details(details)
            .panel(panel)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use formalmusic_api::{LyricLine, LyricWord, Lyrics};

    fn line(start_ms: u64, text: &str) -> LyricLine {
        LyricLine {
            start_ms,
            text: text.into(),
            ..LyricLine::default()
        }
    }

    #[test]
    fn an_empty_line_ends_the_line_before_it() {
        let mut first = line(1_000, "First");
        first.words = vec![
            LyricWord {
                start_ms: 1_000,
                end_ms: 1_400,
                text: "Fir".into(),
                joins_next: true,
            },
            LyricWord {
                start_ms: 1_400,
                end_ms: 2_000,
                text: "st".into(),
                joins_next: false,
            },
        ];
        let mut duet = line(9_000, "Second voice");
        duet.opposite_turn = true;
        duet.background = true;
        let source = Lyrics {
            source: Some("Apple Music".into()),
            lines: vec![first, line(4_000, ""), duet],
            synced: true,
            word_synced: true,
        };
        let out = super::convert(&source);
        assert_eq!(out.lines.len(), 2, "the marker is not a line");
        assert_eq!(
            out.lines[0].end,
            Some(std::time::Duration::from_millis(4_000))
        );
        assert_eq!(out.lines[0].words.len(), 2);
        assert!(out.lines[0].words[0].joins_next);
        assert!(out.lines[1].opposite && out.lines[1].background);
        assert!(out.synced);
        assert_eq!(out.source.as_deref(), Some("Apple Music"));
    }
}
