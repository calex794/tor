//! Successful production navigation cannot append to a legacy individual scope.
use super::*;

pub(super) async fn page_without_collection(
    h: &Harness,
    browser: &Browser,
    actor: i64,
    path: &str,
) -> String {
    let before = optional_snapshot(&h.url, actor);
    let response = browser.get(&h.app, path).await;
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    let html = text(response).await;
    for hook in [
        "data-rec-ticket",
        "data-organic-ticket",
        "/assets/recommendations.js",
        "/assets/organic_discovery.js",
    ] {
        assert!(!html.contains(hook), "{path}: {hook}");
    }
    assert_eq!(optional_snapshot(&h.url, actor), before, "{path}");
    html
}
