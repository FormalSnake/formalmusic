use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const SLUG_MAX_CHARS: usize = 120;

/// `$XDG_CACHE_HOME/formalmusic/<name>`, falling back to the platform cache dir.
pub(crate) fn dir(name: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join("formalmusic").join(name)
}

/// Lowercased alphanumeric runs joined by `-`. Non-Latin titles keep their
/// letters, so two tracks that only differ in script do not share a file.
pub(crate) fn slug(parts: &[&str]) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in parts.iter().flat_map(|p| p.chars().chain([' '])) {
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(c.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out.chars()
        .take(SLUG_MAX_CHARS)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

/// A sibling name for writing a file that is then renamed into place, unique
/// within the process so concurrent writers never share one.
pub(crate) fn temp_sibling(path: &std::path::Path, ext: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{}", NEXT.fetch_add(1, Ordering::Relaxed), ext));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_folds_punctuation_and_case() {
        assert_eq!(
            slug(&["Taylor Swift", "The Life of a Showgirl"]),
            "taylor-swift-the-life-of-a-showgirl"
        );
        assert_eq!(slug(&["  AC/DC ", "Back In Black!"]), "ac-dc-back-in-black");
    }

    #[test]
    fn slug_keeps_non_latin_letters() {
        assert_eq!(slug(&["宇多田ヒカル", "Fantôme"]), "宇多田ヒカル-fantôme");
    }

    #[test]
    fn slug_of_nothing_is_empty() {
        assert_eq!(slug(&["", "  "]), "");
    }
}
