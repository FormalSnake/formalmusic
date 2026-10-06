mod actions;
mod app;
mod art;
mod bridge;
mod chrome;
mod clock;
mod fonts;
mod header;
mod icons;
mod live_theme;
mod menus;
mod motion;
mod new_playlist;
mod now_playing;
mod page;
mod player_bar;
mod primitives;
mod shelves;
mod sidebar;
mod signin;
mod single_instance;
mod theme;
mod toast;
mod topbar;
mod trace;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use app::AppRoot;

/// Wayland app id and X11 `WM_CLASS`; matches the `.desktop` file's name so
/// the shell finds the app's icon by it.
const APP_ID: &str = "es.canarycoders.formalmusic";

fn window_options(cx: &App) -> WindowOptions {
    let mut options = TitleBar::window_options();
    options.titlebar = Some(TitlebarOptions {
        title: Some("FormalMusic".into()),
        traffic_light_position: Some(point(px(16.), px(18.))),
        ..TitleBar::title_bar_options()
    });
    options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
        None,
        size(px(1280.), px(820.)),
        cx,
    )));
    options.window_min_size = Some(size(px(880.), px(560.)));
    options.app_id = Some(APP_ID.into());
    options.focus = std::env::var("GPUIX_BACKGROUND").ok().as_deref() != Some("1");
    options
}

fn main() {
    trace::init();
    let single_instance::Launch::First(activations) = single_instance::claim() else {
        return;
    };
    // Every socket call, timer, file read or write, JSON parse and image
    // decode happens here, never on the GPUI foreground thread.
    // state.json is read and parsed beside platform and window setup, so the
    // first frame paints the last Home, queue and player instead of a spinner.
    let preload = std::thread::Builder::new()
        .name("formalmusic-cache".into())
        .spawn(|| formalmusic_core::cache::StateCache::new(&app::state_dir()).load_blocking())
        .ok();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("formalmusic-rt")
        .enable_all()
        .build()
        .expect("build tokio runtime");
    // Leaked so its Handle stays valid for the process; the runtime itself is
    // never meant to shut down before the process does.
    let runtime_handle = Box::leak(Box::new(runtime)).handle().clone();

    let mut preload = preload;
    gpui_kit::application()
        .with_assets(icons::IconAssets)
        .run(move |cx| {
            cx.set_app_identity(APP_ID, "FormalMusic");
            trace::log_if_enabled("platform up");
            fonts::install(cx);
            // gpui-component resolves ".SystemUIFont" and the platform monospace
            // default by listing every installed font (about 200 ms through
            // CoreText) unless the theme already names families, so both are
            // named first and the default light and dark configs put back after.
            {
                use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
                let mut named = Theme::default();
                named.font_family = theme::font_sans();
                named.mono_font_family = theme::font_sans();
                cx.set_global(named);
                gpui_kit::init(cx);
                let registry = ThemeRegistry::global(cx);
                let (light, dark) = (
                    registry.default_light_theme().clone(),
                    registry.default_dark_theme().clone(),
                );
                let component = Theme::global_mut(cx);
                component.light_theme = light;
                component.dark_theme = dark;
                Theme::change(ThemeMode::Dark, None, cx);
            }
            app::init(cx);
            theme::Theme::install(cx);
            live_theme::watch(cx);
            bridge::Bridge::install(cx);
            trace::watch_keys(cx);
            trace::log_if_enabled("app initialised");

            let runtime_handle = runtime_handle.clone();
            let preload = preload.take();
            cx.spawn(async move |cx| {
                let options = cx.update(|cx| window_options(cx));
                let window = cx
                    .open_window(options, |window, cx| {
                        trace::log_if_enabled("window created");
                        let view = cx.new(|cx| AppRoot::new(runtime_handle, preload, window, cx));
                        trace::log_if_enabled("views built");
                        cx.new(|cx| Root::new(view, window, cx))
                    })
                    .expect("open window");
                let Some(mut activations) = activations else {
                    return;
                };
                while activations.recv().await.is_some() {
                    cx.update(|cx| cx.activate(true));
                    if window
                        .update(cx, |_, window, _| window.activate_window())
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        });
}
