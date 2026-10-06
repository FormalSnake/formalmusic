//! Last.fm's scrobble rule: a track longer than 30 seconds counts once it has
//! played for half its length or four minutes, whichever comes first. Time
//! is wall-clock time spent playing, so pauses and buffering do not count
//! and seeking ahead does not count as listening.

use std::time::{Duration, Instant};

const MIN_LENGTH: Duration = Duration::from_secs(30);
const MAX_WAIT: Duration = Duration::from_secs(240);

/// How long a track has to play before it scrobbles, or `None` when it
/// never does.
pub fn threshold(duration_ms: Option<u64>) -> Option<Duration> {
    let length = Duration::from_millis(duration_ms?);
    (length > MIN_LENGTH).then(|| (length / 2).min(MAX_WAIT))
}

/// One play of one track.
#[derive(Debug)]
pub struct Listen {
    threshold: Option<Duration>,
    played: Duration,
    playing_since: Option<Instant>,
    counted: bool,
}

impl Listen {
    pub fn new(duration_ms: Option<u64>, playing: bool, now: Instant) -> Self {
        Self {
            threshold: threshold(duration_ms),
            played: Duration::ZERO,
            playing_since: playing.then_some(now),
            counted: false,
        }
    }

    /// Returns true when this resumes a paused listen.
    pub fn set_playing(&mut self, playing: bool, now: Instant) -> bool {
        match (playing, self.playing_since) {
            (true, None) => {
                self.playing_since = Some(now);
                true
            }
            (false, Some(since)) => {
                self.played += now.saturating_duration_since(since);
                self.playing_since = None;
                false
            }
            _ => false,
        }
    }

    pub fn played(&self, now: Instant) -> Duration {
        self.played
            + self
                .playing_since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    /// How long until the threshold at the current rate, while it is still
    /// ahead and the track is playing.
    pub fn due_in(&self, now: Instant) -> Option<Duration> {
        let threshold = self.threshold?;
        if self.counted || self.playing_since.is_none() {
            return None;
        }
        Some(threshold.saturating_sub(self.played(now)))
    }

    /// True exactly once, when the threshold has been reached.
    pub fn take_due(&mut self, now: Instant) -> bool {
        let Some(threshold) = self.threshold else {
            return false;
        };
        if self.counted || self.played(now) < threshold {
            return false;
        }
        self.counted = true;
        true
    }

    /// Marks the listen as already counted, for a play the daemon resumed
    /// after it had scrobbled it before a restart.
    pub fn mark_counted(&mut self) {
        self.counted = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    #[test]
    fn threshold_is_half_or_four_minutes() {
        assert_eq!(threshold(Some(30_000)), None);
        assert_eq!(threshold(Some(31_000)), Some(Duration::from_millis(15_500)));
        assert_eq!(threshold(Some(200_000)), Some(100 * S));
        assert_eq!(threshold(Some(480_000)), Some(240 * S));
        assert_eq!(threshold(Some(3_600_000)), Some(240 * S));
        assert_eq!(threshold(None), None);
    }

    #[test]
    fn pauses_do_not_count() {
        let t0 = Instant::now();
        let mut l = Listen::new(Some(200_000), true, t0);
        assert!(!l.set_playing(false, t0 + 60 * S));
        assert!(!l.take_due(t0 + 600 * S));
        assert_eq!(l.due_in(t0 + 600 * S), None);
        assert!(l.set_playing(true, t0 + 600 * S));
        assert_eq!(l.due_in(t0 + 600 * S), Some(40 * S));
        assert!(!l.take_due(t0 + 639 * S));
        assert!(l.take_due(t0 + 640 * S));
    }

    #[test]
    fn counts_once() {
        let t0 = Instant::now();
        let mut l = Listen::new(Some(100_000), true, t0);
        assert!(l.take_due(t0 + 50 * S));
        assert!(!l.take_due(t0 + 90 * S));
        assert_eq!(l.due_in(t0 + 90 * S), None);
    }

    #[test]
    fn short_tracks_never_count() {
        let t0 = Instant::now();
        let mut l = Listen::new(Some(29_000), true, t0);
        assert!(!l.take_due(t0 + 3600 * S));
        assert_eq!(l.due_in(t0), None);
    }

    #[test]
    fn starts_paused_until_playing() {
        let t0 = Instant::now();
        let mut l = Listen::new(Some(100_000), false, t0);
        assert!(!l.take_due(t0 + 100 * S));
        l.set_playing(true, t0 + 100 * S);
        assert!(l.take_due(t0 + 150 * S));
    }

    #[test]
    fn restored_listen_does_not_count_again() {
        let t0 = Instant::now();
        let mut l = Listen::new(Some(100_000), true, t0);
        l.mark_counted();
        assert!(!l.take_due(t0 + 100 * S));
    }
}
