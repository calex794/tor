//! Retired FLOW-03: genuine C reading/library/follow commands remain functional.
//! Current schema 192: live commands create no optional identity or proof.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tor_accounts::{repositories::PgAccounts, services::security::generate_session_token};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tor_recommendations::{
    repositories::flow_funnel::PgFlowFunnel,
    services::flow_funnel::{FlowFunnelCounts, FlowFunnelReport},
};
use tower::ServiceExt;

#[path = "support/collection_retirement.rs"]
mod retirement;
use retirement::{assert_current_schema, optional_snapshot};

#[path = "flow_funnel_http_pg/fixtures.rs"]
mod fixtures;
#[path = "flow_funnel_http_pg/navigation.rs"]
mod navigation;
#[path = "flow_funnel_http_pg/outage.rs"]
mod outage;
#[path = "flow_funnel_http_pg/paid.rs"]
mod paid;
#[path = "flow_funnel_http_pg/proofs.rs"]
mod proofs;
use fixtures::*;

const ORIGIN: &str = "http://127.0.0.1:3098";
const ORDINARY_UA: &str = "Mozilla/5.0 Chrome/132.0.0.0 Safari/537.36";
const FLOW_FORM: &str = "/me/recommendations/flow-consent";
const REC_FORM: &str = "/me/recommendations/consent";

struct Harness {
    app: Router,
    funnel: PgFlowFunnel,
    url: String,
    stamp: u128,
}

async fn setup() -> Harness {
    for name in [
        "TOR_FLOW_VISITS_ENABLED",
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
        "TOR_ORGANIC_ATTRIBUTION_ENABLED",
        "TOR_READING_RETURNS_ENABLED",
        "TOR_BOOK_READING_RETURNS_ENABLED",
        "TOR_RIL_SINGLE_INSTANCE",
        "TOR_FLOW_PUBLIC_OPT_IN_ENABLED",
    ] {
        std::env::set_var(name, "1");
    }
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    assert_current_schema(&url);
    let funnel = PgFlowFunnel::new(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        PgAccounts::from_pool(pool),
        false,
        ORIGIN.to_owned(),
    );
    Harness {
        app,
        funnel,
        url,
        stamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    }
}

fn book(url: &str, name: &str) -> (i64, i64, i64) {
    let id: i64 = pg(
        url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
        VALUES('ci:flow-funnel:'||gen_random_uuid(),'ru','{name}','public','basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
        VALUES({id},1,'FLOW funnel chapter','public','free') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
        VALUES({chapter},1,'abcdefghij') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    (id, chapter, version)
}

async fn progress(
    browser: &Browser,
    app: &Router,
    chapter: i64,
    version: i64,
    offset: i32,
    revision: i64,
    expected: StatusCode,
) {
    let body = json!({"chapter_id":chapter.to_string(),"version_id":version.to_string(),
        "block_id":1,"char_offset":offset,"expected_revision":revision})
    .to_string();
    let response = browser
        .request(
            app,
            "PUT",
            "/api/v1/me/progress/free",
            &body,
            &[
                ("content-type", "application/json"),
                ("x-csrf-token", &browser.cookies["tor-dev-csrf"]),
            ],
        )
        .await;
    assert_eq!(response.status(), expected, "{}", text(response).await);
}

fn metrics(counts: &FlowFunnelCounts) -> [(&'static str, i64); 12] {
    [
        ("cohorts", counts.cohorts),
        ("detail-opens", counts.detail_opens),
        ("reader-opens", counts.reader_opens),
        ("starts", counts.starts),
        ("completions", counts.completions),
        ("saves", counts.saves),
        ("follows", counts.follows),
        ("new-readers", counts.new_readers),
        ("d7-anchors", counts.d7_anchors),
        ("d7-pending", counts.d7_pending),
        ("d7-returned", counts.d7_returned),
        ("d7-unknown", counts.d7_unknown),
    ]
}

fn assert_report(html: &str, report: &FlowFunnelReport) {
    for (key, period) in [("current", &report.current), ("previous", &report.previous)] {
        let marker = format!("data-testid=\"flow-funnel-{key}\"");
        let section = html
            .split(&marker)
            .nth(1)
            .expect("rendered funnel period")
            .split("</section>")
            .next()
            .unwrap();
        assert!(section.contains(&period.from_utc) && section.contains(&period.until_utc));
        if period.counts.is_none() {
            assert!(section.contains("Нет сохранённых посещений книг"));
            assert!(!section.contains("data-metric=\"cohorts\""));
            continue;
        }
        for (metric, value) in metrics(
            period
                .counts
                .as_ref()
                .unwrap_or(&FlowFunnelCounts::default()),
        ) {
            let marker = format!("data-metric=\"{metric}\"");
            let actual = section
                .split(&marker)
                .nth(1)
                .unwrap()
                .split('>')
                .nth(1)
                .unwrap()
                .split('<')
                .next()
                .unwrap();
            assert_eq!(
                actual,
                if period.counts.is_some() {
                    value.to_string()
                } else {
                    "Нет измерения".to_owned()
                }
            );
        }
        for row in &period.rows {
            assert!(section.contains(&format!(
                "data-source=\"{}\" data-discovery=\"{}\"",
                row.acquisition_hint, row.discovery_kind
            )));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn reading_library_and_follows_survive_retirement_without_linked_collection() {
    let h = setup().await;
    let admin_email = format!("funnel-admin-{}@tor-ci.example", h.stamp);
    let (_, admin) = registration(&h.app, &h.url, &admin_email).await;
    let admin_browser = login(&h.app, &admin_email).await;
    let reader_email = format!("funnel-reader-{}@tor-ci.example", h.stamp);
    let (reader_browser, reader) = registration(&h.app, &h.url, &reader_email).await;
    pg(
        &h.url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator')"),
    );
    let first = book(&h.url, &format!("FUNNEL-PRIVATE-{}", h.stamp));
    let viewed = book(&h.url, "FLOW GET-only book");
    assert_current_schema(&h.url);
    let before = optional_snapshot(&h.url, reader);
    for (browser, status) in [
        (&Browser::default(), StatusCode::UNAUTHORIZED),
        (&reader_browser, StatusCode::FORBIDDEN),
    ] {
        let response = browser
            .get(&h.app, "/admin/audience/funnel?days=invalid")
            .await;
        assert_eq!(response.status(), status);
        private(&response);
    }
    let settings = settings(&reader_browser, &h.app).await;
    assert!(!settings.contains(REC_FORM));
    assert!(!settings.contains(FLOW_FORM));
    assert!(!settings.contains("data-flow-consent"));
    for path in [
        format!("/books/{}", first.0),
        format!("/reader/{}", first.1),
        format!("/read/{}", first.1),
        format!("/books/{}", viewed.0),
    ] {
        navigation::page_without_collection(&h, &reader_browser, reader, &path).await;
    }
    assert_eq!(
        pg(
            &h.url,
            &format!("SELECT count(*) FROM reader_chapter_starts WHERE user_id={reader}")
        ),
        "0",
        "a successful GET is not a reading start"
    );
    progress(
        &reader_browser,
        &h.app,
        first.1,
        first.2,
        2,
        0,
        StatusCode::OK,
    )
    .await;
    progress(
        &reader_browser,
        &h.app,
        first.1,
        first.2,
        10,
        1,
        StatusCode::OK,
    )
    .await;
    progress(
        &reader_browser,
        &h.app,
        first.1,
        first.2,
        10,
        2,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        pg(
            &h.url,
            &format!(
                "SELECT
      (SELECT count(*) FROM reader_chapter_starts WHERE user_id={reader})||'|'||
      (SELECT count(*) FROM reader_chapter_completions WHERE user_id={reader})||'|'||
      (SELECT count(*) FROM reader_book_positions WHERE user_id={reader})"
            )
        ),
        "1|1|1"
    );
    assert_eq!(pg(&h.url, &format!("SELECT count(*) FROM outbox_events
      WHERE event_type IN('ChapterStartedV1','ChapterCompletedV1') AND payload->>'user_id'='{reader}'")), "2",
      "canonical C events remain available to their business consumers");
    assert_eq!(optional_snapshot(&h.url, reader), before);

    let body = format!("csrf_token={}", reader_browser.cookies["tor-dev-csrf"]);
    let save_path = format!("/me/library/{}/save", first.0);
    let follow_path = format!("/me/books/{}/follow", first.0);
    assert_eq!(
        reader_browser
            .request(&h.app, "POST", &save_path, "csrf_token=forged", &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        reader_browser
            .request(
                &h.app,
                "POST",
                &follow_path,
                &body,
                &[("origin", "https://invalid.example")]
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        pg(
            &h.url,
            &format!(
                "SELECT
      (SELECT count(*) FROM reader_library_items WHERE user_id={reader})+
      (SELECT count(*) FROM reader_book_follows WHERE user_id={reader})"
            )
        ),
        "0"
    );
    for path in [&save_path, &follow_path, &save_path, &follow_path] {
        assert_eq!(
            reader_browser
                .request(&h.app, "POST", path, &body, &[])
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        pg(
            &h.url,
            &format!(
                "SELECT
      (SELECT count(*) FROM reader_library_items WHERE user_id={reader})||'|'||
      (SELECT count(*) FROM reader_book_follows WHERE user_id={reader})"
            )
        ),
        "1|1"
    );
    let pinned_path = format!("/me/library/saved/{}", first.1);
    navigation::page_without_collection(&h, &reader_browser, reader, &pinned_path).await;
    let guest_pinned = Browser::default().get(&h.app, &pinned_path).await;
    assert_eq!(guest_pinned.status(), StatusCode::SEE_OTHER);
    assert_eq!(guest_pinned.headers()[header::LOCATION], "/login");
    assert_eq!(guest_pinned.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        admin_browser.get(&h.app, &pinned_path).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(
            &h.url,
            &format!("SELECT count(*) FROM reader_chapter_starts WHERE user_id={reader}")
        ),
        "1"
    );
    assert_eq!(
        optional_snapshot(&h.url, reader),
        before,
        "positive C progress/save/follow cannot create personal activity, return or FLOW receipts"
    );
    for endpoint in [REC_FORM, FLOW_FORM] {
        retired_choice(&reader_browser, &h.app, endpoint, false).await;
        retired_choice(&reader_browser, &h.app, endpoint, true).await;
    }
    assert_eq!(optional_snapshot(&h.url, reader), before);
    for days in [1, 7, 30, 90] {
        let response = admin_browser
            .get(&h.app, &format!("/admin/audience/funnel?days={days}"))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        private(&response);
        let html = text(response).await;
        assert!(html.contains("data-testid=\"collection-retired\""));
        assert!(html.contains("data-current-measurement=\"unavailable\""));
        assert!(html.contains("data-measurement=\"legacy-consented\""));
        assert_report(&html, &h.funnel.report(days).await.unwrap());
        for secret in [
            &reader_email,
            &format!("FUNNEL-PRIVATE-{}", h.stamp),
            "abcdefghij",
        ] {
            assert!(!html.contains(secret));
        }
    }
    for invalid in ["0", "2", "31", "invalid"] {
        let response = admin_browser
            .get(&h.app, &format!("/admin/audience/funnel?days={invalid}"))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        private(&response);
    }
    pg(&h.url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    let response = admin_browser.get(&h.app, "/admin/audience/funnel").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    private(&response);
    assert_eq!(optional_snapshot(&h.url, reader), before);
    assert_eq!(
        pg(
            &h.url,
            &format!("SELECT count(*) FROM reader_library_items WHERE user_id={reader}")
        ),
        "1"
    );
    pg(
        &h.url,
        &format!(
            "DELETE FROM flow_book_daily WHERE book_id={};
        UPDATE books SET admin_hold=true WHERE id IN({},{})",
            first.0, first.0, viewed.0
        ),
    );
}
