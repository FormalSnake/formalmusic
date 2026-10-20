//! Store events to views. A view watches the topics it reads once, in its constructor; each
//! event notifies exactly those views, so a position tick re-renders the player and the lyrics,
//! never a page. Events reach the main thread in batches through `nd::system::spawn`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use formalmusic_core::{MusicStore, StoreEvent};
use nd::prelude::*;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

pub type Topic = StoreEvent;

/// Which views read which topics.
#[derive(Default)]
struct Watchers {
    by_topic: HashMap<Topic, Vec<EntityId>>,
}

/// Re-renders `view` whenever `topic` fires.
pub fn watch(cx: &mut App, topic: Topic, view: EntityId) {
    let mut watchers = watchers(cx).borrow_mut();
    let views = watchers.by_topic.entry(topic).or_default();
    if !views.contains(&view) {
        views.push(view);
    }
}

pub fn unwatch(cx: &mut App, topic: &Topic, view: EntityId) {
    if let Some(views) = watchers(cx).borrow_mut().by_topic.get_mut(topic) {
        views.retain(|v| *v != view);
    }
}

fn watchers(cx: &mut App) -> &RefCell<Watchers> {
    if !cx.has_global::<RefCell<Watchers>>() {
        cx.set_global(RefCell::new(Watchers::default()));
    }
    cx.global::<RefCell<Watchers>>()
}

/// What the main thread is told: one event, or "everything may have changed" after the
/// broadcast channel dropped events for a slow reader.
enum Delivery {
    One(StoreEvent),
    All,
}

/// Starts forwarding `store`'s events to the views that watch them.
pub fn install(cx: &mut App, store: &MusicStore) {
    let mut events = store.events();
    let (tx, rx) = unbounded_channel();
    store.runtime().spawn(async move {
        loop {
            let delivery = match events.recv().await {
                Ok(event) => Delivery::One(event),
                Err(RecvError::Lagged(_)) => Delivery::All,
                Err(RecvError::Closed) => return,
            };
            if tx.send(delivery).is_err() {
                return;
            }
        }
    });
    on_main(rx, |delivery, cx| {
        let ids: Vec<EntityId> = match delivery {
            Delivery::One(event) => watchers(cx)
                .borrow()
                .by_topic
                .get(&event)
                .cloned()
                .unwrap_or_default(),
            Delivery::All => watchers(cx)
                .borrow()
                .by_topic
                .values()
                .flatten()
                .copied()
                .collect(),
        };
        for id in ids {
            cx.notify(id);
        }
    });
    let _ = cx;
}

/// Runs `f` on the main thread for every value `rx` yields, a batch at a time, until the
/// sender is gone.
pub fn on_main<T: Send + 'static>(rx: UnboundedReceiver<T>, f: impl FnMut(T, &mut App) + 'static) {
    step(rx, Rc::new(RefCell::new(f)));
}

fn step<T: Send + 'static, F: FnMut(T, &mut App) + 'static>(
    mut rx: UnboundedReceiver<T>,
    f: Rc<RefCell<F>>,
) {
    nd::system::spawn(
        async move {
            let mut batch = Vec::new();
            if let Some(first) = rx.recv().await {
                batch.push(first);
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
            }
            (batch, rx)
        },
        move |(batch, rx), cx| {
            if batch.is_empty() {
                return;
            }
            for value in batch {
                (f.borrow_mut())(value, cx);
            }
            step(rx, f);
        },
    );
}

/// Runs `f` on the main thread after `delay`, on the store's runtime timer.
pub fn after(cx: &App, delay: std::time::Duration, f: impl FnOnce(&mut App) + 'static) {
    let runtime = crate::music(cx).runtime.clone();
    nd::system::spawn(
        async move {
            let _ = runtime.spawn(tokio::time::sleep(delay)).await;
        },
        move |(), cx| f(cx),
    );
}

/// Runs `future` on the store's runtime and `done` with its output on the main thread.
pub fn task<T: Send + 'static>(
    cx: &App,
    future: impl std::future::Future<Output = T> + Send + 'static,
    done: impl FnOnce(T, &mut App) + 'static,
) {
    let handle = crate::music(cx).runtime.spawn(future);
    nd::system::spawn(async move { handle.await.ok() }, move |out, cx| {
        if let Some(out) = out {
            done(out, cx);
        }
    });
}
