//! The "Edit playlist" dialog: title, description and privacy of a playlist
//! you own, and deleting it, which asks once more first.

use formalmusic_api::{BrowseTarget, LibraryTab, PlaylistEdit, Privacy};
use formalmusic_core::{MusicStore, Route};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::primitives::{Button, ButtonKind, overlay_shadows};
use crate::theme::{Palette, Theme, radius, spacing, type_scale};

type OnClose = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// What the playlist is now, so only what changed is sent.
#[derive(Clone)]
pub struct Current {
    pub playlist_id: String,
    pub title: String,
    pub description: String,
    pub privacy: Privacy,
}

pub struct EditPlaylist {
    store: MusicStore,
    current: Current,
    title: Entity<InputState>,
    description: Entity<TextareaState>,
    privacy: Privacy,
    /// The delete button was pressed once and now asks to confirm.
    confirming: bool,
    busy: bool,
    error: Option<SharedString>,
    on_close: OnClose,
}

impl EditPlaylist {
    pub fn new(
        store: MusicStore,
        current: Current,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).default_value(current.title.clone()));
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 6)
                .placeholder("What this playlist is for")
                .default_value(current.description.clone())
        });
        window.focus(&title.focus_handle(cx), cx);
        Self {
            store,
            privacy: current.privacy,
            current,
            title,
            description,
            confirming: false,
            busy: false,
            error: None,
            on_close: std::rc::Rc::new(on_close),
        }
    }

    fn edits(&self, cx: &App) -> Vec<PlaylistEdit> {
        let title = self.title.read(cx).value().trim().to_owned();
        let description = self.description.read(cx).value().trim().to_owned();
        let mut edits = Vec::new();
        if !title.is_empty() && title != self.current.title {
            edits.push(PlaylistEdit::Rename { title });
        }
        if description != self.current.description {
            edits.push(PlaylistEdit::Describe { description });
        }
        if self.privacy != self.current.privacy {
            edits.push(PlaylistEdit::SetPrivacy {
                privacy: self.privacy,
            });
        }
        edits
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.title.read(cx).value().trim().is_empty() {
            self.error = Some("Give the playlist a title.".into());
            cx.notify();
            return;
        }
        let edits = self.edits(cx);
        let (store, playlist_id) = (self.store.clone(), self.current.playlist_id.clone());
        let task = self
            .store
            .runtime()
            .spawn(async move { store.edit_playlist(playlist_id, edits).await });
        self.finish(task, false, window, cx);
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (store, playlist_id) = (self.store.clone(), self.current.playlist_id.clone());
        let task = self
            .store
            .runtime()
            .spawn(async move { store.delete_playlist(playlist_id).await });
        self.finish(task, true, window, cx);
    }

    /// Closes once `task` succeeds, after a delete on the library's
    /// playlists, or shows its error.
    fn finish(
        &mut self,
        task: tokio::task::JoinHandle<Result<(), String>>,
        deleted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let on_close = self.on_close.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Unable to reach the music daemon.".into()));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    on_close(window, cx);
                    if deleted {
                        crate::app::navigate(
                            Route::Browse(BrowseTarget::Library(LibraryTab::Playlists)),
                            cx,
                        );
                    }
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

    fn privacy_picker(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let options = [
            (
                Privacy::Public,
                "Public",
                "Anyone can search for it and play it",
            ),
            (
                Privacy::Unlisted,
                "Unlisted",
                "Anyone with the link can play it",
            ),
            (Privacy::Private, "Private", "Only you can see it"),
        ];
        div()
            .flex()
            .flex_col()
            .gap(spacing::X1)
            .children(options.into_iter().map(|(privacy, label, detail)| {
                let selected = self.privacy == privacy;
                div()
                    .id(SharedString::from(format!("privacy-{label}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(spacing::X3)
                    .px(spacing::X3)
                    .py(spacing::X2)
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .when(selected, |el| el.bg(palette.press_wash))
                    .when(!selected, |el| {
                        el.hover(move |style| style.bg(palette.hover_wash))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.privacy = privacy;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(16.))
                            .flex_shrink_0()
                            .rounded(radius::PILL)
                            .border_1()
                            .border_color(if selected {
                                palette.accent
                            } else {
                                palette.separator
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |el| {
                                el.child(
                                    div().size(px(8.)).rounded(radius::PILL).bg(palette.accent),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(type_scale::BODY.font_size)
                                    .line_height(px(20.))
                                    .text_color(palette.text)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_size(type_scale::CAPTION.font_size)
                                    .line_height(type_scale::CAPTION.line_height)
                                    .text_color(palette.secondary)
                                    .child(detail),
                            ),
                    )
            }))
            .into_any_element()
    }
}

fn label(text: &'static str, palette: Palette) -> impl IntoElement {
    div()
        .text_size(type_scale::CAPTION.font_size)
        .text_color(palette.secondary)
        .child(text)
}

impl Render for EditPlaylist {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::get(cx);
        let (cancel, outside) = (self.on_close.clone(), self.on_close.clone());
        let footer = if self.confirming {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X2)
                .child(
                    div()
                        .flex_grow(1.)
                        .min_w(px(0.))
                        .text_size(type_scale::BODY.font_size)
                        .text_color(palette.text)
                        .child("Delete this playlist? This can't be undone."),
                )
                .child(
                    Button::new("edit-playlist-keep", "Cancel").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.confirming = false;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    Button::new("edit-playlist-delete-confirm", "Delete playlist")
                        .kind(ButtonKind::Danger)
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx))),
                )
        } else {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X2)
                .child(
                    Button::new("edit-playlist-delete", "Delete playlist")
                        .kind(ButtonKind::Danger)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirming = true;
                            cx.notify();
                        })),
                )
                .child(div().flex_grow(1.))
                .child(
                    Button::new("edit-playlist-cancel", "Cancel")
                        .on_click(move |_, window, cx| cancel(window, cx)),
                )
                .child(
                    Button::new(
                        "edit-playlist-save",
                        if self.busy { "Saving\u{2026}" } else { "Save" },
                    )
                    .kind(ButtonKind::Primary)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                )
        };
        div()
            .id("edit-playlist-layer")
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
                    .id("edit-playlist-dialog")
                    .w(px(440.))
                    .max_h(relative(0.85))
                    .overflow_y_scroll()
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
                            .child("Edit playlist"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(spacing::X1)
                            .child(label("Title", palette))
                            .child(Input::new(&self.title).bordered(true)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(spacing::X1)
                            .child(label("Description", palette))
                            .child(Textarea::new(&self.description)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(spacing::X1)
                            .child(label("Privacy", palette))
                            .child(self.privacy_picker(palette, cx)),
                    )
                    .when_some(self.error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_size(type_scale::CAPTION.font_size)
                                .text_color(palette.danger)
                                .child(error),
                        )
                    })
                    .child(footer),
            )
    }
}
