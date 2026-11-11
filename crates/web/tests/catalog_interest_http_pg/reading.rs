use super::fixture::*;
use axum::http::StatusCode;
use std::time::{Duration, Instant};
use tor_accounts::services::security::session_token_digest;

fn state(html: &str, book: i64) -> String {
    row(html, "exposure-row", book)
        .split("data-reading-state=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .into()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL18 tor_ci and migrator URL"]
async fn same_window_page_badges_require_health_sample_and_all_chapter_evidence() {
    let f = Fixture::new().await;
    let route = f.exposure_url();
    for (cookie, expected) in [
        ("", StatusCode::UNAUTHORIZED),
        (f.cookie.as_str(), StatusCode::FORBIDDEN),
    ] {
        f.get(&route, cookie, expected).await;
    }
    f.promote();
    let unknown = f.get(&route, &f.cookie, StatusCode::OK).await;
    assert_eq!(state(&unknown, f.books[1]), "unknown");
    let epoch = f.certify();
    let before = f.fingerprint();
    let html = f.get(&route, &f.cookie, StatusCode::OK).await;
    assert_eq!(metric(&html, "books"), "53");
    assert_eq!(ids(&html, "exposure-row"), f.books[..50]);
    assert_eq!(
        state(&html, f.books[1]),
        "observed-without-verified-reading"
    );
    assert_eq!(state(&html, f.books[0]), "reading-recorded");
    let existing = row(&html, "exposure-row", f.books[0]);
    assert_eq!(metric(&existing, "chapter-starts"), "1");
    assert_eq!(
        metric(&existing, "chapter-completions"),
        "1",
        "the old reader's new chapter must exclude the no-reading-records badge"
    );
    assert_eq!(
        state(&html, f.books[2]),
        "unknown",
        "a missing C receipt cannot be zero"
    );
    assert_eq!(
        metric(
            &row(&html, "exposure-row", f.books[2]),
            "chapter-completions"
        ),
        "неизвестно"
    );
    assert_eq!(
        state(&html, f.books[3]),
        "unknown",
        "DOM19 is below the observed base"
    );
    let next = f
        .get(
            &format!("{route}&after={}", f.books[49]),
            &f.cookie,
            StatusCode::OK,
        )
        .await;
    assert_eq!(ids(&next, "exposure-row"), f.books[50..]);
    assert_eq!(
        metric(&next, "books"),
        "53",
        "current-page diagnostics cannot redefine global totals"
    );
    for book in &f.books[50..] {
        assert_eq!(state(&next, *book), "reading-recorded");
    }
    let filtered = f
        .get(
            &format!("{route}&class=observed"),
            &f.cookie,
            StatusCode::OK,
        )
        .await;
    assert_eq!(metric(&filtered, "books"), "53");
    assert!(!ids(&filtered, "exposure-row").contains(&f.books[3]));
    assert!(
        ids(&filtered, "exposure-row").contains(&f.books[2]),
        "the existing F filter must not silently become a C-known filter"
    );
    assert_eq!(f.fingerprint(), before);
    let held = HeldLock::acquire(&f, "reader_chapter_starts");
    let began = Instant::now();
    let missing = f.get(&route, &f.cookie, StatusCode::OK).await;
    assert!(began.elapsed() < Duration::from_millis(1500));
    assert!(missing.contains("exposure-reading-unavailable"));
    assert_eq!(metric(&missing, "books"), "53");
    assert_eq!(state(&missing, f.books[1]), "unknown");
    assert_eq!(
        metric(&row(&missing, "exposure-row", f.books[1]), "chapter-starts"),
        "неизвестно"
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            f.accounts.session_actor(&session_token_digest(&f.token))
        )
        .await
        .unwrap()
        .unwrap()
        .is_some(),
        "optional C failure must release the shared reader pool before unlock"
    );
    drop(held);
    assert_eq!(
        state(&f.get(&route, &f.cookie, StatusCode::OK).await, f.books[1]),
        "observed-without-verified-reading"
    );
    f.scope(&format!(
        "INSERT INTO flow_observation_health_hourly(epoch,bucket_ms,lost)
        VALUES({epoch},{},true)",
        (f.anchor - 3600) * 1000
    ));
    let lost = f.get(&route, &f.cookie, StatusCode::OK).await;
    assert_eq!(state(&lost, f.books[1]), "unknown");
    sql(
        &f.url,
        &format!("UPDATE books SET admin_hold=true WHERE id={}", f.books[1]),
    );
    let hidden = f.get(&route, &f.cookie, StatusCode::OK).await;
    assert!(!ids(&hidden, "exposure-row").contains(&f.books[1]));
    assert_eq!(metric(&hidden, "books"), "52");
    sql(
        &f.url,
        &format!("DELETE FROM account_roles WHERE user_id={}", f.actor),
    );
    f.get(&route, &f.cookie, StatusCode::FORBIDDEN).await;
    f.assert_boundary();
    f.finish().await;
}
