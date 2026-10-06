use super::*;
use crate::mock::{Mock, route, sequence};

const ARTISTS: &str = include_str!("../../fixtures/itunes_artist_search.json");
const ALBUMS: &str = include_str!("../../fixtures/itunes_albums.json");
const AMP: &str = include_str!("../../fixtures/amp_editorial_video.json");
const MASTER: &str = include_str!("../../fixtures/hls_master.m3u8");
const MEDIA: &str = include_str!("../../fixtures/hls_media.m3u8");

const ASSET_HOST: &str = "https://mvod.itunes.apple.com/itunes-assets/HLSVideo221/v4/dd/ab/ca/ddabca1c-1be1-0ead-ce75-7b905da49719/";
const VARIANT: &str = "/hls/P1189220687_Anull_video_gr240_sdr_768x768";

fn amp_body() -> String {
    AMP.replace(
        &format!("{ASSET_HOST}P1189220687_default.m3u8"),
        "{base}/hls/master.m3u8",
    )
}

fn routes() -> Vec<crate::mock::Route> {
    vec![
        route("/search", 200, ARTISTS),
        route("/lookup", 200, ALBUMS),
        route(
            "/us/album/",
            200,
            r#"<script src="/assets/index~abc123.js"></script>"#,
        ),
        route(
            "/assets/index~abc123.js",
            200,
            r#"var t={a:"eyJabc.def.ghi"};"#,
        ),
        route("/v1/catalog/us/albums/", 200, amp_body()),
        route(
            "/hls/master.m3u8",
            200,
            MASTER.replace(ASSET_HOST, "{base}/hls/"),
        ),
        route(&format!("{VARIANT}.m3u8"), 200, MEDIA),
        route(&format!("{VARIANT}-.mp4"), 200, b"fakemp4".to_vec()),
    ]
}

fn covers(dir: &tempfile::TempDir, mock: &Mock) -> AnimatedCovers {
    let endpoints = Endpoints {
        itunes: mock.base.clone(),
        web_player: mock.base.clone(),
        amp_api: mock.base.clone(),
    };
    AnimatedCovers::build(dir.path().to_owned(), endpoints).unwrap()
}

fn files(dir: &tempfile::TempDir) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir.path())
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn count(mock: &Mock, prefix: &str) -> usize {
    mock.targets()
        .iter()
        .filter(|t| t.starts_with(prefix))
        .count()
}

#[tokio::test]
async fn downloads_the_768_avc1_rendition_into_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(routes()).await;
    let covers = covers(&dir, &mock);

    let path = covers
        .animated_cover("Taylor Swift", "The Life of a Showgirl")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        path,
        dir.path().join("taylor-swift-the-life-of-a-showgirl.mp4")
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"fakemp4");
    assert_eq!(
        files(&dir),
        ["taylor-swift-the-life-of-a-showgirl.mp4"],
        "no .part left behind"
    );

    let amp = mock.heads("/v1/").remove(0).to_lowercase();
    assert!(
        amp.contains("authorization: bearer eyjabc.def.ghi"),
        "{amp}"
    );
    assert!(amp.contains(&format!("origin: {}", mock.base)), "{amp}");
    assert!(
        mock.targets()
            .iter()
            .any(|t| t.contains("extend=editorialVideo"))
    );
    let search = mock.targets().remove(0);
    assert!(
        search.contains("entity=musicArtist") && search.contains("term=Taylor+Swift"),
        "{search}"
    );

    let before = mock.targets().len();
    assert_eq!(
        covers
            .animated_cover("Taylor Swift", "The Life of a Showgirl")
            .await
            .unwrap(),
        Some(path)
    );
    assert_eq!(
        mock.targets().len(),
        before,
        "a file on disk needs no network"
    );
}

#[tokio::test]
async fn the_token_is_scraped_once_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::start(routes()).await;
    let covers = covers(&dir, &mock);
    covers
        .animated_cover("Taylor Swift", "The Life of a Showgirl")
        .await
        .unwrap()
        .unwrap();
    covers
        .animated_cover("Taylor Swift", "Midnights")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count(&mock, "/assets/"), 1);
    assert_eq!(count(&mock, "/v1/"), 2);
}

#[tokio::test]
async fn an_expired_token_is_rescraped_and_the_request_retried() {
    let dir = tempfile::tempdir().unwrap();
    let mut routes = routes();
    routes.retain(|r| !r.matches("/v1/"));
    routes.push(sequence(
        "/v1/",
        vec![(401, Vec::new()), (200, amp_body().into_bytes())],
    ));
    let mock = Mock::start(routes).await;
    let found = covers(&dir, &mock)
        .animated_cover("Taylor Swift", "The Life of a Showgirl")
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(count(&mock, "/assets/"), 2);
    assert_eq!(count(&mock, "/v1/"), 2);
}

#[tokio::test]
async fn a_token_the_server_keeps_refusing_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut routes = routes();
    routes.retain(|r| !r.matches("/v1/"));
    routes.push(route("/v1/", 403, ""));
    let mock = Mock::start(routes).await;
    assert!(
        covers(&dir, &mock)
            .animated_cover("Taylor Swift", "The Life of a Showgirl")
            .await
            .is_err()
    );
    assert_eq!(count(&mock, "/v1/"), 2);
    assert!(files(&dir).is_empty());
}

#[tokio::test]
async fn an_album_without_editorial_video_is_a_remembered_miss() {
    let dir = tempfile::tempdir().unwrap();
    let mut routes = routes();
    routes.retain(|r| !r.matches("/v1/"));
    routes.push(route(
        "/v1/",
        200,
        r#"{"data":[{"attributes":{"name":"x"}}]}"#,
    ));
    let mock = Mock::start(routes).await;
    let covers = covers(&dir, &mock);
    assert_eq!(
        covers
            .animated_cover("Taylor Swift", "Midnights")
            .await
            .unwrap(),
        None
    );
    let before = mock.targets().len();
    assert_eq!(
        covers
            .animated_cover("Taylor Swift", "Midnights")
            .await
            .unwrap(),
        None
    );
    assert_eq!(mock.targets().len(), before);
    assert!(files(&dir).is_empty());
}

#[tokio::test]
async fn unknown_artist_album_or_catalog_entry_is_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let mut routes = routes();
    routes.retain(|r| !r.matches("/v1/"));
    routes.push(route("/v1/", 404, ""));
    let mock = Mock::start(routes).await;
    let covers = covers(&dir, &mock);
    assert_eq!(
        covers
            .animated_cover("Nobody At All", "Midnights")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        covers
            .animated_cover("Taylor Swift", "No Such Album")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        covers
            .animated_cover("Taylor Swift", "Midnights")
            .await
            .unwrap(),
        None
    );
    assert_eq!(covers.animated_cover("", "").await.unwrap(), None);
}

#[tokio::test]
async fn a_server_error_is_retried_next_time_rather_than_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let mut routes = routes();
    routes.retain(|r| !r.matches("/search"));
    routes.push(sequence(
        "/search",
        vec![(503, Vec::new()), (200, ARTISTS.as_bytes().to_vec())],
    ));
    let mock = Mock::start(routes).await;
    let covers = covers(&dir, &mock);
    assert!(
        covers
            .animated_cover("Taylor Swift", "Midnights")
            .await
            .is_err()
    );
    assert!(
        covers
            .animated_cover("Taylor Swift", "Midnights")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_failed_download_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let mut routes = routes();
    routes.retain(|r| !r.matches(&format!("{VARIANT}-.mp4")));
    routes.push(route(&format!("{VARIANT}-.mp4"), 200, Vec::new()));
    let mock = Mock::start(routes).await;
    assert!(
        covers(&dir, &mock)
            .animated_cover("Taylor Swift", "Midnights")
            .await
            .is_err()
    );
    assert!(files(&dir).is_empty(), "{:?}", files(&dir));
}

#[test]
fn thirty_days_exactly_is_not_yet_stale() {
    let now = SystemTime::now();
    assert!(!is_stale(now - MAX_AGE, now));
    assert!(is_stale(now - MAX_AGE - Duration::from_secs(1), now));
    assert!(
        !is_stale(now + Duration::from_secs(60), now),
        "a future mtime is never stale"
    );
}

#[tokio::test]
async fn prune_removes_only_stale_files() {
    let dir = tempfile::tempdir().unwrap();
    for (name, age_days) in [("old.mp4", 31), ("old.mp4.1.part", 40), ("fresh.mp4", 2)] {
        let path = dir.path().join(name);
        std::fs::write(&path, "x").unwrap();
        let modified = SystemTime::now() - Duration::from_secs(age_days * 24 * 60 * 60);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }
    prune(dir.path()).await;
    assert_eq!(files(&dir), ["fresh.mp4"]);
    prune(&dir.path().join("missing")).await;
}
