//! Sheets on the main window: sign in, new playlist, edit playlist and the keyboard shortcuts.

use formalmusic_api::{BrowseTarget, Browsers, LibraryTab, PlaylistEdit, Privacy, ProfileBrowser};
use formalmusic_core::Route;
use nd::prelude::*;

use crate::bridge;
use crate::root;

const PASTE_STEPS: [&str; 4] = [
    "Open music.youtube.com in your browser and sign in.",
    "Open the developer tools (F12), then the Network tab, and reload the page.",
    "Select any request to music.youtube.com and find Cookie under Request Headers.",
    "Copy the whole value and paste it below.",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Browser,
    Paste,
}

pub struct SignIn {
    mode: Mode,
    profiles: Vec<ProfileBrowser>,
    browsers: Option<Browsers>,
    browser: usize,
    cookies: SharedString,
    busy: bool,
    waiting_on: Option<String>,
    error: Option<String>,
}

pub fn sign_in(window: &mut Window, cx: &mut App) {
    window.open_sheet_sized(cx, size(px(560.), px(560.)), |_, cx| cx.new(SignIn::new));
}

impl SignIn {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = crate::store(cx);
        let page = cx.weak_entity();
        let (a, b) = (store.clone(), store);
        bridge::task(cx, async move { a.browser_profiles().await }, {
            let page = page.clone();
            move |profiles, cx| {
                page.update(cx, |this, cx| {
                    this.profiles = profiles.unwrap_or_default();
                    cx.notify();
                });
            }
        });
        bridge::task(
            cx,
            async move { b.browsers().await },
            move |browsers, cx| {
                page.update(cx, |this, cx| {
                    if let Ok(browsers) = browsers {
                        this.browser = browsers
                            .default
                            .as_ref()
                            .and_then(|d| browsers.installed.iter().position(|b| b.id == *d))
                            .unwrap_or(0);
                        this.browsers = Some(browsers);
                    }
                    cx.notify();
                });
            },
        );
        Self {
            mode: Mode::Browser,
            profiles: Vec::new(),
            browsers: None,
            browser: 0,
            cookies: SharedString::default(),
            busy: false,
            waiting_on: None,
            error: None,
        }
    }

    /// Runs one sign-in path; on success says who signed in and closes the sheet.
    fn start(
        &mut self,
        work: impl std::future::Future<Output = Result<String, String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        self.error = None;
        let page = cx.weak_entity();
        bridge::task(cx, work, move |result, cx| {
            page.update(cx, |this, cx| {
                this.busy = false;
                this.waiting_on = None;
                match result {
                    Ok(name) => {
                        crate::store(cx).notice(if name.is_empty() {
                            "Signed in".into()
                        } else {
                            format!("Signed in as {name}")
                        });
                        root::dismiss_sign_in(cx);
                        root::close_sheets(cx);
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        });
        cx.notify();
    }
}

impl Render for SignIn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = crate::store(cx);
        let signed_in = store.state().signed_in();
        let mut body = v_flex().gap_3().child(
            label(if signed_in {
                "Your library, likes and recommendations come from your account. The session stays on this computer."
            } else {
                "Your library, likes and recommendations come from your account. The cookies stay on this computer."
            })
            .secondary(),
        );
        match self.mode {
            Mode::Browser => {
                if !self.profiles.is_empty() {
                    body = body.child(
                        label("Use a browser you're already signed in to")
                            .text_style(TextStyle::Headline),
                    );
                    for (b, browser) in self.profiles.iter().enumerate() {
                        for (p, profile) in browser.profiles.iter().enumerate() {
                            let title = match &profile.email {
                                Some(email) => format!(
                                    "{} \u{2022} {} ({email})",
                                    browser.browser.name, profile.name
                                ),
                                None => {
                                    format!("{} \u{2022} {}", browser.browser.name, profile.name)
                                }
                            };
                            let (browser_id, path) =
                                (browser.browser.id.clone(), profile.path.clone());
                            body = body.child(
                                button(format!("signin.profile.{b}.{p}"), title)
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        let store = crate::store(cx);
                                        let (browser, profile) = (browser_id.clone(), path.clone());
                                        this.start(
                                                async move {
                                                    store.import_cookies(browser, profile).await
                                                },
                                                cx,
                                            );
                                    })),
                            );
                        }
                    }
                    body =
                        body.child(label("Or sign in to Google").text_style(TextStyle::Headline));
                }
                match &self.browsers {
                    Some(browsers) if !browsers.installed.is_empty() => {
                        let names: Vec<SharedString> = browsers.installed.iter().map(|b| b.name.clone().into()).collect();
                        let ids: Vec<String> = browsers.installed.iter().map(|b| b.id.clone()).collect();
                        let chosen = ids.get(self.browser).cloned();
                        body = body
                            .child(select("signin.browser", names).selected(self.browser).on_change(cx.listener(
                                |this, e: &SelectChanged, _, cx| {
                                    this.browser = e.index;
                                    cx.notify();
                                },
                            )))
                            .child(match &self.waiting_on {
                                Some(name) => v_flex()
                                    .gap_2()
                                    .child(label(format!("Finish signing in in the {name} window.")))
                                    .child(button("signin.cancel", "Cancel").on_click(|_, _, cx| {
                                        let store = crate::store(cx);
                                        bridge::task(cx, async move { store.cancel_sign_in().await }, |_, _| {});
                                    }))
                                    .into_any_element(),
                                None => button("signin.browser-go", "Sign in with a new browser window")
                                    .accent()
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        let store = crate::store(cx);
                                        let browser = chosen.clone();
                                        this.waiting_on = this
                                            .browsers
                                            .as_ref()
                                            .and_then(|b| b.installed.get(this.browser))
                                            .map(|b| b.name.clone());
                                        this.start(async move { store.browser_sign_in(browser).await.map(|()| String::new()) }, cx);
                                    }))
                                    .into_any_element(),
                            });
                    }
                    Some(_) => {
                        body = body.child(label(
                            "No supported browser found. Install Firefox, Chromium or Helium, or paste cookies instead.",
                        ))
                    }
                    None => body = body.child(spinner()),
                }
                body = body.child(
                    button("signin.paste-mode", "Paste cookies instead")
                        .borderless()
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.mode = Mode::Paste;
                            cx.notify();
                        })),
                );
            }
            Mode::Paste => {
                for (i, step) in PASTE_STEPS.iter().enumerate() {
                    body = body.child(label(format!("{}. {step}", i + 1)));
                }
                body = body
                    .child(
                        text_area("signin.cookies")
                            .placeholder("SID=...; HSID=...; SAPISID=...; __Secure-3PSID=...")
                            .text(self.cookies.clone())
                            .label("Cookie header")
                            .on_change(cx.listener(|this, e: &TextChanged, _, cx| {
                                this.cookies = e.text.clone();
                                cx.notify();
                            }))
                            .h(px(96.)),
                    )
                    .child(
                        button(
                            "signin.paste-go",
                            if self.busy {
                                "Signing in\u{2026}"
                            } else {
                                "Sign in"
                            },
                        )
                        .accent()
                        .disabled(self.busy)
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                let cookies = this.cookies.trim().to_owned();
                                if cookies.is_empty() {
                                    this.error = Some("Paste the Cookie header first.".into());
                                    cx.notify();
                                    return;
                                }
                                let store = crate::store(cx);
                                this.start(
                                async move { store.sign_in(cookies).await.map(|()| String::new()) },
                                cx,
                            );
                            },
                        )),
                    )
                    .child(
                        button("signin.browser-mode", "Sign in with your browser instead")
                            .borderless()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.mode = Mode::Browser;
                                cx.notify();
                            })),
                    );
            }
        }
        if let Some(error) = &self.error {
            body = body.child(label(error.clone()).tone(LabelTone::Danger));
        }
        sheet_layout("Sign in to YouTube Music")
            .w(px(560.))
            .child(body)
            .footer(
                h_flex().gap_2().child(
                    button(
                        "signin.skip",
                        if signed_in {
                            "Close"
                        } else {
                            "Browse without signing in"
                        },
                    )
                    .on_click(|_, window, cx| {
                        root::dismiss_sign_in(cx);
                        window.close_sheet(cx);
                    }),
                ),
            )
    }
}

pub struct NewPlaylist {
    title: SharedString,
    busy: bool,
    error: Option<String>,
}

pub fn new_playlist(window: &mut Window, cx: &mut App) {
    window.open_sheet_sized(cx, size(px(420.), px(220.)), |_, cx| {
        cx.new(|_| NewPlaylist {
            title: SharedString::default(),
            busy: false,
            error: None,
        })
    });
}

impl NewPlaylist {
    fn create(&mut self, cx: &mut Context<Self>) {
        let title = self.title.trim().to_owned();
        if title.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        let store = crate::store(cx);
        let page = cx.weak_entity();
        bridge::task(
            cx,
            async move { store.create_playlist(title, Vec::new()).await },
            move |result, cx| {
                page.update(cx, |this, cx| {
                    this.busy = false;
                    match result {
                        Ok(id) => {
                            root::close_sheets(cx);
                            root::navigate(Route::Browse(BrowseTarget::Playlist(id)), cx);
                        }
                        Err(e) => {
                            this.error = Some(if e.is_empty() {
                                "Could not create the playlist.".into()
                            } else {
                                e
                            })
                        }
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
    }
}

impl Render for NewPlaylist {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        sheet_layout("New playlist")
            .child(
                text_field("new-playlist.title")
                    .label("Title")
                    .placeholder("Road trip")
                    .text(self.title.clone())
                    .on_change(cx.listener(|this, e: &TextChanged, _, cx| {
                        this.title = e.text.clone();
                        cx.notify();
                    }))
                    .on_submit(cx.listener(|this, _: &TextSubmitted, _, cx| this.create(cx))),
            )
            .children(self.error.clone().map(|e| label(e).tone(LabelTone::Danger)))
            .footer(
                h_flex()
                    .gap_2()
                    .child(
                        button("new-playlist.cancel", "Cancel")
                            .on_click(|_, window, cx| window.close_sheet(cx)),
                    )
                    .child(
                        button("new-playlist.create", "Create playlist")
                            .primary()
                            .disabled(self.busy || self.title.trim().is_empty())
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.create(cx))),
                    ),
            )
    }
}

/// A playlist as the edit sheet opens it.
#[derive(Clone, Debug)]
pub struct EditPlaylist {
    pub playlist_id: String,
    pub title: String,
    pub description: String,
    pub privacy: Privacy,
}

pub struct EditPlaylistSheet {
    original: EditPlaylist,
    title: SharedString,
    description: SharedString,
    privacy: Privacy,
    busy: bool,
    error: Option<String>,
}

const PRIVACY: [(Privacy, &str); 3] = [
    (
        Privacy::Public,
        "Public: anyone can search for it and play it",
    ),
    (
        Privacy::Unlisted,
        "Unlisted: anyone with the link can play it",
    ),
    (Privacy::Private, "Private: only you can see it"),
];

pub fn edit_playlist(current: EditPlaylist, window: &mut Window, cx: &mut App) {
    window.open_sheet_sized(cx, size(px(480.), px(380.)), move |_, cx| {
        cx.new(move |_| EditPlaylistSheet {
            title: current.title.clone().into(),
            description: current.description.clone().into(),
            privacy: current.privacy,
            original: current,
            busy: false,
            error: None,
        })
    });
}

impl EditPlaylistSheet {
    fn save(&mut self, cx: &mut Context<Self>) {
        let title = self.title.trim().to_owned();
        if title.is_empty() {
            self.error = Some("Give the playlist a title.".into());
            cx.notify();
            return;
        }
        let mut edits = Vec::new();
        if title != self.original.title {
            edits.push(PlaylistEdit::Rename { title });
        }
        if self.description.as_ref() != self.original.description {
            edits.push(PlaylistEdit::Describe {
                description: self.description.to_string(),
            });
        }
        if self.privacy != self.original.privacy {
            edits.push(PlaylistEdit::SetPrivacy {
                privacy: self.privacy,
            });
        }
        if edits.is_empty() {
            root::close_sheets(cx);
            return;
        }
        self.busy = true;
        let (store, id) = (crate::store(cx), self.original.playlist_id.clone());
        let page = cx.weak_entity();
        bridge::task(
            cx,
            async move { store.edit_playlist(id, edits).await },
            move |result, cx| {
                page.update(cx, |this, cx| {
                    this.busy = false;
                    match result {
                        Ok(()) => root::close_sheets(cx),
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.original.playlist_id.clone();
        let page = cx.weak_entity();
        window.confirm(
            cx,
            "Delete playlist",
            Some(self.original.title.clone().into()),
            "Delete playlist",
            move |yes, cx| {
                if !yes {
                    return;
                }
                let store = crate::store(cx);
                bridge::task(
                    cx,
                    async move { store.delete_playlist(id).await },
                    move |result, cx| match result {
                        Ok(()) => {
                            root::close_sheets(cx);
                            root::navigate(
                                Route::Browse(BrowseTarget::Library(LibraryTab::Playlists)),
                                cx,
                            );
                        }
                        Err(e) => {
                            page.update(cx, |this, cx| {
                                this.error = Some(e);
                                cx.notify();
                            });
                        }
                    },
                );
            },
        );
    }
}

impl Render for EditPlaylistSheet {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let privacy_index = PRIVACY
            .iter()
            .position(|(p, _)| *p == self.privacy)
            .unwrap_or(2);
        sheet_layout("Edit playlist")
            .child(
                text_field("edit-playlist.title")
                    .label("Title")
                    .text(self.title.clone())
                    .on_change(cx.listener(|this, e: &TextChanged, _, cx| {
                        this.title = e.text.clone();
                        cx.notify();
                    })),
            )
            .child(
                text_area("edit-playlist.description")
                    .label("Description")
                    .placeholder("What this playlist is for")
                    .text(self.description.clone())
                    .on_change(cx.listener(|this, e: &TextChanged, _, cx| {
                        this.description = e.text.clone();
                        cx.notify();
                    }))
                    .h(px(72.)),
            )
            .child(
                radio_group("edit-playlist.privacy", PRIVACY.map(|(_, l)| l))
                    .selected(privacy_index)
                    .on_change(cx.listener(|this, e: &SelectChanged, _, cx| {
                        this.privacy = PRIVACY[e.index.min(2)].0;
                        cx.notify();
                    })),
            )
            .children(self.error.clone().map(|e| label(e).tone(LabelTone::Danger)))
            .footer(
                h_flex()
                    .gap_2()
                    .child(
                        button("edit-playlist.delete", "Delete playlist")
                            .destructive()
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.delete(window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        button("edit-playlist.cancel", "Cancel")
                            .on_click(|_, window, cx| window.close_sheet(cx)),
                    )
                    .child(
                        button(
                            "edit-playlist.save",
                            if self.busy { "Saving\u{2026}" } else { "Save" },
                        )
                        .primary()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save(cx))),
                    ),
            )
    }
}

/// The `?` sheet: every single-key shortcut, as music.youtube.com has them.
pub struct Shortcuts;

pub fn shortcuts(window: &mut Window, cx: &mut App) {
    window.open_sheet_sized(cx, size(px(520.), px(620.)), |_, cx| cx.new(|_| Shortcuts));
}

pub const SHORTCUTS: [(&str, &str); 20] = [
    ("Space, ;", "Play or pause"),
    ("J, Shift+N", "Next"),
    ("K, Shift+P", "Previous"),
    ("L, Shift+Right", "Forward 10 seconds"),
    ("H, Shift+Left", "Back 10 seconds"),
    ("Shift+L", "Forward 1 second"),
    ("Shift+H", "Back 1 second"),
    ("=, Shift+Up", "Volume up"),
    ("-, Shift+Down", "Volume down"),
    ("M", "Mute"),
    ("S", "Shuffle"),
    ("R", "Repeat"),
    ("+", "Like"),
    ("_", "Dislike"),
    ("/", "Search"),
    ("Q", "Up next"),
    ("F", "Expanded player"),
    ("G H, G E, G L", "Home, Explore, Library"),
    ("G ,", "Settings"),
    ("?", "These shortcuts"),
];

impl Render for Shortcuts {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        sheet_layout("Keyboard shortcuts")
            .child(
                form().child(
                    form_section().children(
                        SHORTCUTS
                            .iter()
                            .map(|(keys, what)| form_row(*what).child(label(*keys).secondary())),
                    ),
                ),
            )
            .footer(
                button("shortcuts.done", "Done")
                    .primary()
                    .on_click(|_, window, cx| window.close_sheet(cx)),
            )
    }
}
