//! How well a search hit matches the track being played.

use std::collections::HashSet;

/// A candidate under this score is dropped outright.
pub(crate) const MATCH_SCORE_FLOOR: f64 = 55.0;
/// A candidate further than this from the track's length loses regardless of
/// how well its text matches.
pub(crate) const DURATION_WINDOW_SECS: f64 = 12.0;

/// Lowercased, "(feat." and friends stripped so a featured artist does not
/// cost a match its score, split on anything that is not ASCII alphanumeric,
/// duplicates collapsed.
fn tokens(value: &str) -> Vec<String> {
    let lowered = value
        .to_lowercase()
        .replace("(feat.", " ")
        .replace("(ft.", " ")
        .replace("(featuring", " ");
    let mut seen = HashSet::new();
    lowered
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty() && seen.insert(*t))
        .map(str::to_owned)
        .collect()
}

/// Twice the shared token count over the combined token count, as a
/// percentage. Zero when either side has no tokens.
pub(crate) fn match_score(candidate: &str, query: &str) -> f64 {
    let candidate = tokens(candidate);
    let query = tokens(query);
    if candidate.is_empty() || query.is_empty() {
        return 0.0;
    }
    let shared = candidate.iter().filter(|t| query.contains(t)).count();
    (2 * shared) as f64 * 100.0 / (candidate.len() + query.len()) as f64
}

/// Combined score, or `None` when the candidate is dropped: under the floor,
/// or both lengths are known and further apart than the window.
pub(crate) fn rank(
    text_score: f64,
    track_ms: Option<u64>,
    candidate_secs: Option<f64>,
) -> Option<f64> {
    if text_score < MATCH_SCORE_FLOOR {
        return None;
    }
    let mut duration_score = 0.0;
    if let (Some(track_ms), Some(candidate)) = (track_ms.filter(|ms| *ms > 0), candidate_secs) {
        let delta = (candidate - track_ms as f64 / 1000.0).abs();
        if delta > DURATION_WINDOW_SECS {
            return None;
        }
        duration_score = DURATION_WINDOW_SECS - delta;
    }
    Some(text_score + duration_score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_scores_100() {
        assert_eq!(
            match_score("Anti-Hero Taylor Swift", "anti hero taylor swift"),
            100.0
        );
    }

    #[test]
    fn featured_artist_marker_is_ignored_but_names_count() {
        let with_feat = match_score("Song (feat. Someone) Artist", "Song Artist");
        assert!(with_feat > 70.0, "{with_feat}");
    }

    #[test]
    fn unrelated_text_scores_zero_and_empty_is_zero() {
        assert_eq!(
            match_score("Blank Space Taylor Swift", "Espresso Sabrina Carpenter"),
            0.0
        );
        assert_eq!(match_score("", "x"), 0.0);
        assert_eq!(match_score("---", "x"), 0.0);
    }

    #[test]
    fn duplicates_collapse() {
        assert_eq!(match_score("la la la", "la"), 100.0);
    }

    #[test]
    fn floor_drops_weak_matches() {
        assert_eq!(rank(54.9, None, None), None);
        assert_eq!(rank(55.0, None, None), Some(55.0));
    }

    #[test]
    fn duration_inside_window_adds_closeness() {
        assert_eq!(rank(100.0, Some(200_000), Some(200.0)), Some(112.0));
        assert_eq!(rank(100.0, Some(200_000), Some(206.0)), Some(106.0));
        assert_eq!(rank(100.0, Some(200_000), Some(212.0)), Some(100.0));
    }

    #[test]
    fn duration_outside_window_drops_regardless_of_text() {
        assert_eq!(rank(100.0, Some(200_000), Some(212.5)), None);
    }

    #[test]
    fn unknown_durations_do_not_penalise() {
        assert_eq!(rank(80.0, None, Some(500.0)), Some(80.0));
        assert_eq!(rank(80.0, Some(200_000), None), Some(80.0));
        assert_eq!(rank(80.0, Some(0), Some(500.0)), Some(80.0));
    }
}
