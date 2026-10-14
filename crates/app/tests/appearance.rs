//! Tier 2: the appearance audit (`nd::audit`) over the window on the demo catalog, in light,
//! dark and matugen themes: every colour is a theme token and every text run reads.

fn main() {
    nd::audit::Audit::new("formalmusic", formalmusic::app)
        .window("main", |cx| formalmusic::open_demo(cx).id())
        .window("lyrics", |cx| {
            let window = formalmusic::open_demo(cx);
            window.root().update(cx, |root, cx| {
                root.set_expanded(Some(formalmusic::now_playing::Tab::Lyrics), cx)
            });
            window.id()
        })
        .run()
}
