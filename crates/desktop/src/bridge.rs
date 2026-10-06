//! The change-notification path: a `StoreEvent` broadcast turns into a
//! `cx.notify()` on only the entities watching the topic it touched.
//!
//! One foreground task (`Bridge::drain`) owns the store's broadcast receiver.
//! Views never read the channel themselves; they call `Bridge::watch(cx,
//! topic, cx.entity().downgrade().into())` once, in their constructor.

use std::collections::HashMap;

use formalmusic_core::{MusicStore, StoreEvent};
use gpui_kit::{AnyWeakEntity, App, Global};

/// Topics are the store's events one to one: `Position` reaches the seek
/// bar alone, `Page(target)` only the view showing that page.
pub type Topic = StoreEvent;

/// Registry of who watches what. A GPUI [`Global`]: every access is from the
/// foreground thread, so no lock is needed.
#[derive(Default)]
pub struct Bridge {
    watchers: HashMap<Topic, Vec<AnyWeakEntity>>,
    /// Bumped by every `drain`, so the task of a store that was replaced stops.
    epoch: u64,
}

impl Global for Bridge {}

impl Bridge {
    pub fn install(cx: &mut App) {
        cx.set_global(Bridge::default());
    }

    /// Registers interest in `topic`. Call once per topic from the watching
    /// view's constructor with `cx.entity().downgrade().into()`.
    pub fn watch(cx: &mut App, topic: Topic, entity: AnyWeakEntity) {
        let entities = cx.global_mut::<Bridge>().watchers.entry(topic).or_default();
        if !entities
            .iter()
            .any(|weak| weak.entity_id() == entity.entity_id())
        {
            entities.push(entity);
        }
    }

    /// Drops `entity`'s interest in `topic`, so a view that moves between
    /// pages stops repainting for the ones it left.
    pub fn unwatch(cx: &mut App, topic: &Topic, entity: &AnyWeakEntity) {
        let watchers = &mut cx.global_mut::<Bridge>().watchers;
        let Some(entities) = watchers.get_mut(topic) else {
            return;
        };
        entities.retain(|weak| weak.entity_id() != entity.entity_id() && weak.is_upgradable());
        if entities.is_empty() {
            watchers.remove(topic);
        }
    }

    /// Notifies `topic`'s watchers for a change the store did not send,
    /// such as the clock moving the position on between events.
    pub fn notify(cx: &mut App, topic: &Topic) {
        Bridge::dispatch(cx, topic);
    }

    fn dispatch(cx: &mut App, topic: &Topic) {
        let watchers = &mut cx.global_mut::<Bridge>().watchers;
        let Some(entities) = watchers.get_mut(topic) else {
            return;
        };
        entities.retain(|weak| weak.is_upgradable());
        let entities = entities.clone();
        if entities.is_empty() {
            watchers.remove(topic);
        }
        for weak in entities {
            cx.notify(weak.entity_id());
        }
    }

    fn dispatch_all(cx: &mut App) {
        let topics: Vec<Topic> = cx.global::<Bridge>().watchers.keys().cloned().collect();
        for topic in topics {
            Bridge::dispatch(cx, &topic);
        }
    }

    /// Runs for the life of the store: one task draining its broadcast
    /// receiver and turning each event into `cx.notify()` calls.
    pub fn drain(cx: &mut App, store: MusicStore) {
        cx.set_global(StoreHandle(Some(store.clone())));
        let epoch = {
            let bridge = cx.global_mut::<Bridge>();
            bridge.epoch += 1;
            bridge.epoch
        };
        cx.spawn(async move |cx| {
            let mut events = store.events();
            drop(store);
            loop {
                let event = events.recv().await;
                if cx.update(|cx| cx.global::<Bridge>().epoch != epoch) {
                    break;
                }
                match event {
                    Ok(event) => cx.update(|cx| {
                        Bridge::dispatch(cx, &event);
                        match event {
                            StoreEvent::NowPlaying => crate::clock::Clock::sync(cx),
                            StoreEvent::Position | StoreEvent::Lyrics(_) => {
                                crate::clock::Clock::update_line(cx)
                            }
                            _ => {}
                        }
                    }),
                    // A lagged receiver missed events; the safe answer is
                    // "everything changed", so every registered view repaints once.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        cx.update(Bridge::dispatch_all)
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .detach();
    }
}

/// The current store. A [`Global`] so any view can reach it without threading
/// it through every constructor.
#[derive(Clone, Default)]
pub struct StoreHandle(pub Option<MusicStore>);

impl Global for StoreHandle {}

pub fn store(cx: &App) -> Option<MusicStore> {
    cx.try_global::<StoreHandle>()
        .and_then(|handle| handle.0.clone())
}
