//! The "New playlist" dialog: a title, then the new playlist opens.

use formalmusic_core::model::BrowseTarget;
use formalmusic_core::{MusicStore, Route};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::primitives::{Button, ButtonKind, overlay_shadows};
use crate::theme::{Theme, radius, spacing, type_scale};

pub struct NewPlaylist {
    store: MusicStore,
    input: Entity<InputState>,
    error: Option<SharedString>,
    busy: bool,
    on_close: std::rc::Rc<dyn Fn(&mut Window, &mut App)>,
    _subscription: Subscription,
}

impl NewPlaylist {
    pub fn new(
        store: MusicStore,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Road trip"));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.create(window, cx);
                }
            },
        );
        window.focus(&input.focus_handle(cx), cx);
        Self {
            store,
            input,
            error: None,
            busy: false,
            on_close: std::rc::Rc::new(on_close),
            _subscription: subscription,
        }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.input.read(cx).value().trim().to_owned();
        if title.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let store = self.store.clone();
        let task = store.runtime().spawn({
            let store = store.clone();
            async move { store.create_playlist(title, Vec::new()).await }
        });
        let on_close = self.on_close.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Could not create the playlist.".into()));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(playlist_id) => {
                    on_close(window, cx);
                    crate::app::navigate(Route::Browse(BrowseTarget::Playlist(playlist_id)), cx);
                }
                Err(error) => {
                    this.busy = false;
                    this.error = Some(error.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for NewPlaylist {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::get(cx);
        let on_close = self.on_close.clone();
        let outside = self.on_close.clone();
        div()
            .id("new-playlist-layer")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(palette.scrim)
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| outside(window, cx))
            .child(
                div()
                    .id("new-playlist-dialog")
                    .w(px(400.))
                    .p(spacing::X6)
                    .flex()
                    .flex_col()
                    .gap(spacing::X4)
                    .rounded(radius::CARD)
                    .bg(palette.overlay)
                    .border_1()
                    .border_color(palette.overlay_border)
                    .shadow(overlay_shadows(&palette))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_size(type_scale::LARGE.font_size)
                            .line_height(type_scale::LARGE.line_height)
                            .font_weight(FontWeight::BOLD)
                            .text_color(palette.text)
                            .child("New playlist"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(spacing::X1)
                            .child(
                                div()
                                    .text_size(type_scale::CAPTION.font_size)
                                    .text_color(palette.secondary)
                                    .child("Title"),
                            )
                            .child(Input::new(&self.input).bordered(true)),
                    )
                    .when_some(self.error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_size(type_scale::CAPTION.font_size)
                                .text_color(palette.danger)
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(spacing::X2)
                            .child(
                                Button::new("new-playlist-cancel", "Cancel")
                                    .on_click(move |_, window, cx| on_close(window, cx)),
                            )
                            .child(
                                Button::new(
                                    "new-playlist-create",
                                    if self.busy {
                                        "Creating\u{2026}"
                                    } else {
                                        "Create playlist"
                                    },
                                )
                                .kind(ButtonKind::Primary)
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.create(window, cx)),
                                ),
                            ),
                    ),
            )
    }
}
