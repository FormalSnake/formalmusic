//! Text the UI shows that is worth testing on its own.

use formalmusic_api::{Link, Track};

/// `3:07`, `1:02:45`.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// "Nadia Reyes & Kofi Mensah", the way bylines read on the web app.
pub fn names(links: &[Link]) -> String {
    match links {
        [] => String::new(),
        [one] => one.text.clone(),
        [rest @ .., last] => format!(
            "{} & {}",
            rest.iter()
                .map(|link| link.text.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            last.text
        ),
    }
}

/// Artists, then the album, as a track row's second line.
pub fn byline(track: &Track) -> String {
    let artists = names(&track.artists);
    match &track.album {
        Some(album) if !artists.is_empty() => format!("{artists} \u{2022} {}", album.text),
        Some(album) => album.text.clone(),
        None => artists,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_the_web_app() {
        assert_eq!(duration(0), "0:00");
        assert_eq!(duration(187_400), "3:07");
        assert_eq!(duration(3_765_000), "1:02:45");
    }

    #[test]
    fn several_artists_join_with_an_ampersand() {
        let link = |text: &str| Link {
            text: text.into(),
            target: None,
        };
        assert_eq!(names(&[link("A")]), "A");
        assert_eq!(names(&[link("A"), link("B"), link("C")]), "A, B & C");
    }
}
