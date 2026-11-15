use super::fixture::*;
use axum::http::StatusCode;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL18 tor_ci and migrator URL"]
async fn full_scope_top_twenty_preserves_keysets_authority_filters_and_small_samples() {
    let f = Fixture::new().await;
    let route = f.rank_url();
    for (cookie, expected) in [
        ("", StatusCode::UNAUTHORIZED),
        (f.cookie.as_str(), StatusCode::FORBIDDEN),
    ] {
        let html = f
            .get(
                "/admin/audience/reading?view=top20&days=bad",
                cookie,
                expected,
            )
            .await;
        assert!(!html.contains("data-metric="));
    }
    f.promote();
    for query in [
        "view=unknown",
        "view=top20&after_id=1",
        "view=top20&kind=tags&after_label=x",
        "view=top20&reader=1",
        "view=top20&view=all",
    ] {
        f.get(
            &format!("/admin/audience/reading?{query}"),
            &f.cookie,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let before = f.fingerprint();
    let all = format!("/admin/audience/reading?days=7&tag={}", f.tag);
    let first = f.get(&all, &f.cookie, StatusCode::OK).await;
    assert_eq!(ids(&first, "canonical-row"), f.books[..20]);
    assert_eq!(metric(&first, "books"), "53");
    assert_eq!(metric(&first, "current"), "29");
    let next = f
        .get(
            &format!("{all}&after_id={}", f.books[19]),
            &f.cookie,
            StatusCode::OK,
        )
        .await;
    assert_eq!(ids(&next, "canonical-row"), f.books[20..40]);
    assert_eq!(metric(&next, "current"), "29");
    let html = f.get(&route, &f.cookie, StatusCode::OK).await;
    let ranked = ids(&html, "canonical-rank-row");
    assert_eq!(ranked.len(), 20);
    assert_eq!(&ranked[..3], &f.books[50..53]);
    assert_eq!(metric(&html, "books"), "53");
    assert_eq!(metric(&html, "current"), "29");
    assert!(!html.contains("canonical-next"));
    assert!(html.contains("&lt;img src=x"));
    assert!(!html.contains("<img src=x"));
    assert!(!html.contains(&f.token));
    assert!(!row(&html, "canonical-rank-row", f.books[50]).contains("canonical-rank-small-current"));
    assert!(row(&html, "canonical-rank-row", f.books[52]).contains("canonical-rank-small-current"));
    assert!(
        !row(&html, "canonical-rank-row", f.books[51]).contains("canonical-rank-small-previous")
    );
    assert_eq!(
        metric(&row(&first, "canonical-row", f.books[0]), "current"),
        "0",
        "an old account/book completion must not create another ranked pair"
    );
    for kind in ["genres", "tags"] {
        let grouped = f
            .get(&format!("{route}&kind={kind}"), &f.cookie, StatusCode::OK)
            .await;
        assert_eq!(metric(&grouped, "books"), "53");
        assert_eq!(metric(&grouped, "current"), "29");
        assert!(grouped.contains("canonical-ranking"));
    }
    let filtered = f
        .get(&format!("{route}&genre=shared"), &f.cookie, StatusCode::OK)
        .await;
    assert_eq!(ids(&filtered, "canonical-rank-row"), f.books[50..52]);
    assert_eq!(metric(&filtered, "books"), "2");
    assert_eq!(metric(&filtered, "current"), "20");
    assert_eq!(
        f.fingerprint(),
        before,
        "GET must not mutate any C/F facts or acknowledgement"
    );
    sql(
        &f.url,
        &format!("UPDATE books SET admin_hold=true WHERE id={}", f.books[50]),
    );
    let hidden = f.get(&route, &f.cookie, StatusCode::OK).await;
    assert!(!ids(&hidden, "canonical-rank-row").contains(&f.books[50]));
    assert_eq!(metric(&hidden, "books"), "52");
    let held = HeldLock::acquire(&f, "outbox_events");
    let began = Instant::now();
    let missing = f
        .get(&route, &f.cookie, StatusCode::SERVICE_UNAVAILABLE)
        .await;
    assert!(began.elapsed() < Duration::from_millis(1500));
    assert!(missing.contains("canonical-rank-unavailable"));
    assert!(!missing.contains("data-metric="));
    drop(held);
    f.get(&route, &f.cookie, StatusCode::OK).await;
    sql(
        &f.url,
        &format!("DELETE FROM account_roles WHERE user_id={}", f.actor),
    );
    f.get(&route, &f.cookie, StatusCode::FORBIDDEN).await;
    sql(
        &f.url,
        &format!(
            "UPDATE sessions SET revoked_at=now() WHERE user_id={}",
            f.actor
        ),
    );
    f.get(&route, &f.cookie, StatusCode::UNAUTHORIZED).await;
    f.finish().await;
}
