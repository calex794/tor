//! Real C write from the parent journey, read-only scoped report and live metadata.
use super::*;
fn value(html: &str, key: &str) -> i64 {
    html.split("data-testid=\"canonical-totals\"")
        .nth(1)
        .unwrap()
        .split(&format!("data-metric=\"{key}\">"))
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}
pub(super) async fn check(
    app: &Router,
    url: &str,
    cookies: &[String],
    book: i64,
    stamp: u128,
    source: &str,
) {
    let tag = format!("c-http-{stamp}");
    pg(url,&format!("UPDATE books SET tags=ARRAY['{tag}'],genres=ARRAY['canonical-http'],title='<img src=x onerror=alert(1)>' WHERE id={book}"));
    let route = format!("/admin/audience/reading?tag={tag}");
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (cookies[1].as_str(), StatusCode::FORBIDDEN),
        (cookies[2].as_str(), StatusCode::FORBIDDEN),
    ] {
        let response = call(
            app,
            "GET",
            "/admin/audience/reading?days=invalid",
            cookie,
            "",
            "",
        )
        .await;
        assert_eq!(response.status(), status);
        private(&response);
        assert!(!page(response).await.contains("data-metric="));
    }
    for query in [
        "days=0",
        "days=2",
        "kind=accounts",
        "kind=books&after_label=x",
        "kind=tags&after_id=1",
        "genre=a&without_genre=true",
        "export=true",
    ] {
        let response = call(
            app,
            "GET",
            &format!("/admin/audience/reading?{query}"),
            &cookies[0],
            "",
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        private(&response);
    }
    let raw_before=pg(url,"SELECT (SELECT count(*) FROM rec_subjects)||'|'||(SELECT count(*) FROM reader_chapter_completions)||'|'||(SELECT count(*) FROM outbox_events)");
    for days in [1, 7, 30, 90] {
        for kind in ["books", "genres", "tags"] {
            let response = call(
                app,
                "GET",
                &format!("{route}&days={days}&kind={kind}"),
                &cookies[0],
                "",
                "",
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            private(&response);
            assert!(response.headers().contains_key("content-security-policy"));
            let html = page(response).await;
            assert_eq!(value(&html, "books"), 1);
            assert_eq!(value(&html, "current"), 1);
            assert_eq!(value(&html, "previous"), 0);
            assert_eq!(value(&html, "library"), 1);
            assert_eq!(value(&html, "follows"), 1);
            assert!(html.contains("canonical-small"));
            assert!(!html.contains("<img src=x"));
            if kind == "books" {
                assert!(html.contains("&lt;img src=x"));
            }
            for secret in [source, "Audience text", "audience-reader-"] {
                assert!(!html.contains(secret));
            }
        }
    }
    assert_eq!(pg(url,"SELECT (SELECT count(*) FROM rec_subjects)||'|'||(SELECT count(*) FROM reader_chapter_completions)||'|'||(SELECT count(*) FROM outbox_events)"),raw_before);
    pg(
        url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    let hidden = page(call(app, "GET", &route, &cookies[0], "", "").await).await;
    for key in ["books", "current", "previous", "library", "follows"] {
        assert_eq!(value(&hidden, key), 0);
    }
    assert!(!hidden.contains("&lt;img src=x"));
    pg(url,&format!("UPDATE books SET admin_hold=false,title='AUDIENCE-HIDDEN-TITLE-{stamp}' WHERE id={book}"));
    let owner = std::env::var("MIGRATOR_URL").unwrap();
    let lock = super::intent::LockedFollows::acquire(&owner);
    let start = std::time::Instant::now();
    let unavailable = call(app, "GET", &route, &cookies[0], "", "").await;
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    private(&unavailable);
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    let unavailable = page(unavailable).await;
    assert!(unavailable.contains("canonical-unavailable"));
    assert!(!unavailable.contains("data-metric="));
    drop(lock);
    let recovered = page(call(app, "GET", &route, &cookies[0], "", "").await).await;
    assert_eq!(value(&recovered, "current"), 1);
}
