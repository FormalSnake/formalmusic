//! The play queue as a plain data structure: order, current entry, shuffle and
//! repeat. No I/O, so every rule here is unit tested.

use formalmusic_api::{EnqueuePosition, Rating, Repeat, Track};
use serde::{Deserialize, Serialize};

/// Radio tops the queue up once fewer than this many tracks follow the current one.
pub const RADIO_LOW_WATER: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Stable across moves and removals, unlike the index.
    pub uid: u64,
    pub track: Track,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Queue {
    entries: Vec<Entry>,
    current: Option<usize>,
    next_uid: u64,
    /// The unshuffled order, by uid, while shuffle is on.
    original: Option<Vec<u64>>,
    pub repeat: Repeat,
}

/// What a removal did to the current entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removed {
    Other,
    /// The playing entry went away; the one now at `current` (if any) should play.
    Current,
}

impl Queue {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn tracks(&self) -> Vec<Track> {
        self.entries.iter().map(|e| e.track.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.current?)
    }

    pub fn index_of(&self, uid: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.uid == uid)
    }

    pub fn shuffled(&self) -> bool {
        self.original.is_some()
    }

    /// Tracks after the current one, ignoring repeat.
    pub fn remaining(&self) -> usize {
        match self.current {
            Some(current) => self.entries.len() - current - 1,
            None => self.entries.len(),
        }
    }

    fn wrap(&mut self, tracks: Vec<Track>) -> Vec<Entry> {
        tracks
            .into_iter()
            .map(|track| {
                self.next_uid += 1;
                Entry {
                    uid: self.next_uid,
                    track,
                }
            })
            .collect()
    }

    /// Replaces the queue. With `shuffle`, the `start` track plays first when
    /// it is not the first one, and everything else is shuffled.
    pub fn replace(
        &mut self,
        tracks: Vec<Track>,
        start: usize,
        shuffle: bool,
        rng: &mut fastrand::Rng,
    ) {
        self.entries = self.wrap(tracks);
        self.original = None;
        self.current = (!self.entries.is_empty()).then(|| start.min(self.entries.len() - 1));
        if shuffle {
            let keep_first = start > 0;
            self.shuffle_on(rng, keep_first);
        }
    }

    pub fn set_shuffle(&mut self, on: bool, rng: &mut fastrand::Rng) {
        match (on, self.original.is_some()) {
            (true, false) => self.shuffle_on(rng, true),
            (false, true) => self.shuffle_off(),
            _ => {}
        }
    }

    /// Shuffles everything but the current entry, which moves to the front so
    /// the whole shuffled order still lies ahead.
    fn shuffle_on(&mut self, rng: &mut fastrand::Rng, keep_current: bool) {
        self.original = Some(self.entries.iter().map(|e| e.uid).collect());
        let current = match (keep_current, self.current) {
            (true, Some(index)) => Some(self.entries.remove(index)),
            _ => None,
        };
        rng.shuffle(&mut self.entries);
        if let Some(current) = current {
            self.entries.insert(0, current);
        }
        self.current = (!self.entries.is_empty()).then_some(0);
    }

    fn shuffle_off(&mut self) {
        let Some(original) = self.original.take() else {
            return;
        };
        let current_uid = self.current().map(|e| e.uid);
        let mut rest = std::mem::take(&mut self.entries);
        for uid in original {
            if let Some(i) = rest.iter().position(|e| e.uid == uid) {
                self.entries.push(rest.remove(i));
            }
        }
        self.entries.append(&mut rest);
        self.current = current_uid.and_then(|uid| self.index_of(uid));
    }

    /// Returns true when the queue was empty, so the caller starts playback.
    pub fn enqueue(&mut self, tracks: Vec<Track>, position: EnqueuePosition) -> bool {
        let was_empty = self.entries.is_empty();
        let new = self.wrap(tracks);
        let uids: Vec<u64> = new.iter().map(|e| e.uid).collect();
        let at = match position {
            EnqueuePosition::Next => self.current.map_or(0, |c| c + 1),
            EnqueuePosition::End => self.entries.len(),
        };
        let current_uid = self.current().map(|e| e.uid);
        if let Some(original) = &mut self.original {
            let at = match (position, current_uid) {
                (EnqueuePosition::Next, Some(uid)) => original
                    .iter()
                    .position(|u| *u == uid)
                    .map_or(original.len(), |i| i + 1),
                (EnqueuePosition::Next, None) => 0,
                (EnqueuePosition::End, _) => original.len(),
            };
            original.splice(at..at, uids);
        }
        self.entries.splice(at..at, new);
        if was_empty && !self.entries.is_empty() {
            self.current = Some(0);
        }
        was_empty
    }

    /// Appends radio tracks, skipping ones already queued.
    pub fn append_radio(&mut self, tracks: Vec<Track>) -> usize {
        let fresh: Vec<Track> = tracks
            .into_iter()
            .filter(|t| !self.entries.iter().any(|e| e.track.video_id == t.video_id))
            .collect();
        let added = fresh.len();
        let was_empty = self.entries.is_empty();
        self.enqueue(fresh, EnqueuePosition::End);
        if was_empty {
            self.current = None;
        }
        added
    }

    /// Appends the next page of the list that is playing. While shuffled,
    /// the new tracks land at random among the ones still to come, as if the
    /// whole list had been shuffled at the start.
    pub fn extend(&mut self, tracks: Vec<Track>, rng: &mut fastrand::Rng) {
        let new = self.wrap(tracks);
        let Some(original) = &mut self.original else {
            self.entries.extend(new);
            return;
        };
        original.extend(new.iter().map(|e| e.uid));
        let first = self.current.map_or(0, |c| c + 1);
        for entry in new {
            let at = rng.usize(first..=self.entries.len());
            self.entries.insert(at, entry);
        }
    }

    /// Sets the like state of every entry for `video_id`; true when one changed.
    pub fn set_like(&mut self, video_id: &str, like: Rating) -> bool {
        let mut changed = false;
        for entry in self.entries.iter_mut() {
            if entry.track.video_id == video_id && entry.track.like != Some(like) {
                entry.track.like = Some(like);
                changed = true;
            }
        }
        changed
    }

    pub fn remove(&mut self, index: usize) -> Option<Removed> {
        if index >= self.entries.len() {
            return None;
        }
        let entry = self.entries.remove(index);
        if let Some(original) = &mut self.original {
            original.retain(|u| *u != entry.uid);
        }
        let removed = match self.current {
            Some(current) if current > index => {
                self.current = Some(current - 1);
                Removed::Other
            }
            Some(current) if current == index => {
                self.current =
                    (!self.entries.is_empty()).then(|| index.min(self.entries.len() - 1));
                Removed::Current
            }
            _ => Removed::Other,
        };
        Some(removed)
    }

    pub fn move_entry(&mut self, from: usize, to: usize) -> bool {
        if from >= self.entries.len() || to >= self.entries.len() {
            return false;
        }
        let current_uid = self.current().map(|e| e.uid);
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        self.current = current_uid.and_then(|uid| self.index_of(uid));
        true
    }

    /// Drops everything but the current entry, as the web app's "Clear queue" does.
    pub fn clear(&mut self) {
        let current = self.current.map(|c| self.entries.remove(c));
        self.entries = current.into_iter().collect();
        self.current = (!self.entries.is_empty()).then_some(0);
        self.original = self
            .original
            .as_ref()
            .map(|_| self.entries.iter().map(|e| e.uid).collect());
    }

    pub fn jump(&mut self, index: usize) -> bool {
        if index < self.entries.len() {
            self.current = Some(index);
            true
        } else {
            false
        }
    }

    /// The entry after the current one. `auto` is a track running out, where
    /// repeat one replays it; a skip always moves on.
    pub fn next_index(&self, auto: bool) -> Option<usize> {
        let current = self.current?;
        if auto && self.repeat == Repeat::One {
            return Some(current);
        }
        if current + 1 < self.entries.len() {
            Some(current + 1)
        } else if self.repeat != Repeat::Off && !self.entries.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    pub fn previous_index(&self) -> Option<usize> {
        let current = self.current?;
        if current > 0 {
            Some(current - 1)
        } else if self.repeat == Repeat::All {
            Some(self.entries.len() - 1)
        } else {
            None
        }
    }

    /// Up to `n` entries that will play after the current one, in order.
    pub fn upcoming(&self, n: usize) -> Vec<&Entry> {
        let Some(current) = self.current else {
            return Vec::new();
        };
        let len = self.entries.len();
        let steps = match self.repeat {
            Repeat::One => return vec![&self.entries[current]],
            Repeat::All => len,
            Repeat::Off => len - current - 1,
        };
        (1..=steps)
            .map(|step| &self.entries[(current + step) % len])
            .take(n)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use formalmusic_api::TrackKind;

    fn track(id: &str) -> Track {
        Track {
            video_id: id.into(),
            title: id.to_uppercase(),
            artists: Vec::new(),
            album: None,
            duration_ms: Some(180_000),
            thumbnails: Vec::new(),
            explicit: false,
            kind: TrackKind::Song,
            like: None,
            set_video_id: None,
            plays: None,
            feedback_token: None,
            counterpart: None,
        }
    }

    fn queue(ids: &str, start: usize) -> Queue {
        let mut q = Queue::default();
        q.replace(
            ids.split(' ').map(track).collect(),
            start,
            false,
            &mut fastrand::Rng::with_seed(1),
        );
        q
    }

    fn ids(q: &Queue) -> String {
        q.entries()
            .iter()
            .map(|e| e.track.video_id.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn current(q: &Queue) -> &str {
        &q.current().unwrap().track.video_id
    }

    #[test]
    fn shuffle_keeps_current_first_and_restores_order() {
        let mut q = queue("a b c d e f g h", 3);
        let mut rng = fastrand::Rng::with_seed(7);
        q.set_shuffle(true, &mut rng);
        assert!(q.shuffled());
        assert_eq!(q.current_index(), Some(0));
        assert_eq!(current(&q), "d");
        assert_ne!(ids(&q), "d a b c e f g h");
        let mut sorted: Vec<_> = ids(&q).split(' ').map(str::to_owned).collect();
        sorted.sort();
        assert_eq!(sorted.join(" "), "a b c d e f g h");

        // Play next while shuffled lands right after current in both orders.
        q.enqueue(vec![track("x")], EnqueuePosition::Next);
        assert_eq!(q.get_id(1), "x");
        q.jump(4);
        let playing = current(&q).to_owned();
        q.set_shuffle(false, &mut rng);
        assert_eq!(ids(&q), "a b c d x e f g h");
        assert_eq!(current(&q), playing);
    }

    #[test]
    fn shuffled_play_starts_with_the_chosen_track() {
        let mut q = Queue::default();
        q.replace(
            "a b c d".split(' ').map(track).collect(),
            2,
            true,
            &mut fastrand::Rng::with_seed(3),
        );
        assert_eq!(current(&q), "c");
        assert_eq!(q.current_index(), Some(0));
    }

    #[test]
    fn repeat_modes() {
        let mut q = queue("a b c", 2);
        assert_eq!(q.next_index(true), None);
        assert_eq!(q.upcoming(2).len(), 0);
        q.repeat = Repeat::All;
        assert_eq!(q.next_index(true), Some(0));
        assert_eq!(
            q.upcoming(2)
                .iter()
                .map(|e| e.track.video_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        q.jump(0);
        assert_eq!(q.previous_index(), Some(2));
        q.repeat = Repeat::One;
        assert_eq!(q.next_index(true), Some(0), "a track running out replays");
        assert_eq!(q.next_index(false), Some(1), "a skip moves on");
        assert_eq!(q.upcoming(2).len(), 1);
        q.repeat = Repeat::Off;
        assert_eq!(q.previous_index(), None);
    }

    #[test]
    fn move_follows_the_current_entry() {
        let mut q = queue("a b c d", 1);
        assert!(q.move_entry(1, 3));
        assert_eq!(ids(&q), "a c d b");
        assert_eq!(current(&q), "b");
        assert!(q.move_entry(0, 2));
        assert_eq!(ids(&q), "c d a b");
        assert_eq!(q.current_index(), Some(3));
        assert!(!q.move_entry(0, 9));
    }

    #[test]
    fn remove_and_clear() {
        let mut q = queue("a b c d", 2);
        assert_eq!(q.remove(0), Some(Removed::Other));
        assert_eq!(current(&q), "c");
        assert_eq!(q.remove(1), Some(Removed::Current));
        assert_eq!(current(&q), "d");
        assert_eq!(q.remove(1), Some(Removed::Current));
        assert_eq!(current(&q), "b", "removing the last entry falls back one");
        assert_eq!(q.remove(5), None);
        let mut q = queue("a b c d", 2);
        q.clear();
        assert_eq!(ids(&q), "c");
        assert_eq!(q.current_index(), Some(0));
    }

    #[test]
    fn enqueue_into_an_empty_queue_starts_it() {
        let mut q = Queue::default();
        assert!(q.enqueue(vec![track("a"), track("b")], EnqueuePosition::End));
        assert_eq!(current(&q), "a");
        assert!(!q.enqueue(vec![track("c")], EnqueuePosition::Next));
        assert_eq!(ids(&q), "a c b");
    }

    #[test]
    fn a_like_lands_on_every_entry_of_the_video() {
        let mut q = queue("a b a", 0);
        assert!(q.set_like("a", Rating::Like));
        assert!(!q.set_like("a", Rating::Like));
        let likes: Vec<_> = q.entries().iter().map(|e| e.track.like).collect();
        assert_eq!(likes, [Some(Rating::Like), None, Some(Rating::Like)]);
    }

    #[test]
    fn radio_appends_only_new_tracks() {
        let mut q = queue("a b c", 0);
        assert_eq!(q.remaining(), 2);
        assert!(q.remaining() < RADIO_LOW_WATER);
        let added = q.append_radio(vec![track("b"), track("d"), track("e")]);
        assert_eq!(added, 2);
        assert_eq!(ids(&q), "a b c d e");
        assert_eq!(current(&q), "a");
        let uids: Vec<u64> = q.entries().iter().map(|e| e.uid).collect();
        let mut unique = uids.clone();
        unique.dedup();
        assert_eq!(uids, unique);
    }

    #[test]
    fn later_pages_shuffle_in_after_the_current_track() {
        let mut q = queue("a b c", 1);
        let mut rng = fastrand::Rng::with_seed(5);
        q.extend(vec![track("d"), track("e")], &mut rng);
        assert_eq!(ids(&q), "a b c d e");

        let mut q = queue("a b c d", 0);
        q.set_shuffle(true, &mut rng);
        q.jump(1);
        let played: Vec<String> = q.entries()[..=1]
            .iter()
            .map(|e| e.track.video_id.clone())
            .collect();
        q.extend("e f g h i j".split(' ').map(track).collect(), &mut rng);
        assert_eq!(q.len(), 10);
        assert_eq!(q.current_index(), Some(1));
        let start: Vec<&str> = q.entries()[..=1]
            .iter()
            .map(|e| e.track.video_id.as_str())
            .collect();
        assert_eq!(start, played, "nothing lands before the current track");
        assert!(!ids(&q).ends_with("e f g h i j"), "{}", ids(&q));
        q.set_shuffle(false, &mut rng);
        assert_eq!(ids(&q), "a b c d e f g h i j");
    }

    #[test]
    fn survives_a_round_trip_through_json() {
        let mut q = queue("a b c", 1);
        q.set_shuffle(true, &mut fastrand::Rng::with_seed(2));
        q.repeat = Repeat::All;
        let back: Queue = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(ids(&back), ids(&q));
        assert_eq!(back.current_index(), q.current_index());
        assert!(back.shuffled());
        assert_eq!(back.repeat, Repeat::All);
    }

    impl Queue {
        fn get_id(&self, index: usize) -> &str {
            &self.entries[index].track.video_id
        }
    }
}
