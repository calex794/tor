//! Retained source aggregates stay historical while ordinary navigation and C reading continue.
//! Positive report rows below are explicit old-model fixtures, never evidence of new capture.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use serde_json::{json, Value};
use std::{
    num::NonZeroU32,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tor_recommendations::{
    repositories::audience::PgDiscoveryAudience,
    services::audience::{DiscoveryReport, DiscoverySource},
};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let output = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        )
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            sql,
        ])
        .output()
        .expect("disposable PostgreSQL client");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    auth: (&str, &str),
    body: &str,
    json_body: bool,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, auth.0)
                .header(header::ORIGIN, ORIGIN)
                .header("x-csrf-token", auth.1)
                .header(
                    header::CONTENT_TYPE,
                    if json_body {
                        "application/json"
                    } else {
                        "application/x-www-form-urlencoded"
                    },
                )
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn private(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let robots = response.headers()["x-robots-tag"].to_str().unwrap();
    assert!(robots.contains("noindex") && robots.contains("nofollow"));
}

fn source_key(source: DiscoverySource) -> &'static str {
    match source {
        DiscoverySource::Catalog => "catalog",
        DiscoverySource::Search => "search",
        DiscoverySource::Recommendations => "recommendations",
        DiscoverySource::Unattributed => "unattributed",
    }
}

fn assert_report(html: &str, report: &DiscoveryReport) {
    for (key, period) in [("current", &report.current), ("previous", &report.previous)] {
        let marker = format!("data-testid=\"audience-sources-{key}\"");
        let section = html.split(marker.as_str()).nth(1).expect("source period");
        for boundary in [&period.from_day, &period.until_day_exclusive] {
            assert!(section.contains(&format!("datetime=\"{boundary}\"")));
        }
        for row in &period.channels {
            let marker = format!("data-source=\"{}\"", source_key(row.source));
            let card = section
                .split(marker.as_str())
                .nth(1)
                .expect("source card")
                .split("data-source=")
                .next()
                .unwrap();
            for (metric, value) in [
                ("clicks", row.clicks),
                ("book-opens", row.book_opens),
                ("starts", row.starts),
                ("completions", row.completions),
            ] {
                let marker = format!("data-metric=\"{metric}\"");
                let rendered = card
                    .split(marker.as_str())
                    .nth(1)
                    .expect("metric")
                    .split('>')
                    .nth(1)
                    .unwrap()
                    .split('<')
                    .next()
                    .unwrap()
                    .trim();
                match value {
                    Some(number) => assert_eq!(rendered, number.to_string()),
                    None if row.source == DiscoverySource::Unattributed
                        && ["clicks", "book-opens"].contains(&metric) =>
                    {
                        assert_eq!(rendered, "Не измеряется");
                    }
                    None => assert_eq!(rendered, "Нет данных"),
                }
            }
        }
    }
}

fn add_chapter(url: &str, book: i64, number: i32) -> (i64, i64) {
    let chapter: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES({book},{number},'Source chapter','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES({chapter},1,'Readable text') RETURNING id"
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
    (chapter, version)
}

async fn complete(app: &Router, auth: (&str, &str), chapter: (i64, i64)) {
    let state = call(
        app,
        "GET",
        &format!("/api/v1/me/progress/free?chapter_id={}", chapter.0),
        auth,
        "",
        false,
    )
    .await;
    assert_eq!(state.status(), StatusCode::OK);
    let mut revision = serde_json::from_str::<Value>(&text(state).await).unwrap()["revision"]
        .as_i64()
        .unwrap();
    for (index, offset) in [3, 13, 13].into_iter().enumerate() {
        let expected = if index == 2 {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        let response = call(
            app,
            "PUT",
            "/api/v1/me/progress/free",
            auth,
            &json!({
                "chapter_id":chapter.0.to_string(), "version_id":chapter.1.to_string(),
                "block_id":1, "char_offset":offset, "expected_revision":revision,
            })
            .to_string(),
            true,
        )
        .await;
        assert_eq!(response.status(), expected);
        if expected == StatusCode::OK {
            revision = serde_json::from_str::<Value>(&text(response).await).unwrap()["revision"]
                .as_i64()
                .unwrap();
        }
    }
}

fn optional_capture(url: &str, reader: i64, books: &[i64]) -> String {
    let ids = books
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    pg(url, &format!(
        "SELECT
         (SELECT count(*) FROM rec_assignments WHERE book_id IN({ids}))
        +(SELECT count(*) FROM rec_attribution_events e JOIN rec_assignments a ON a.id=e.assignment_id WHERE a.book_id IN({ids}))
        +(SELECT count(*) FROM rec_first_conversions WHERE book_id IN({ids}))
        +(SELECT count(*) FROM rec_reader_signals WHERE book_id IN({ids}))
        +(SELECT count(*) FROM organic_discovery_touches WHERE book_id IN({ids}))
        +(SELECT count(*) FROM organic_discovery_first_conversions WHERE book_id IN({ids}))
        +(SELECT count(*) FROM organic_discovery_unattributed_reads WHERE book_id IN({ids}))
        +(SELECT count(*) FROM flow_visit_receipts r JOIN rec_subjects s USING(subject_id) WHERE s.user_id={reader})
        +(SELECT count(*) FROM flow_book_cohorts WHERE book_id IN({ids}))
        +(SELECT count(*) FROM reader_flow_read_receipts r JOIN rec_subjects s USING(subject_id) WHERE s.user_id={reader})
        +(SELECT count(*) FROM reader_flow_book_actions WHERE book_id IN({ids}))
        +(SELECT count(*) FROM reader_flow_activity_days WHERE book_id IN({ids}))
        +(SELECT count(*) FROM event_deliveries d JOIN outbox_events e ON e.id=d.event_id
          WHERE e.payload->>'user_id'='{reader}'
            AND d.consumer IN('recommendation-feedback-v1','flow-read-attribution-v1'))"
    ))
}

async fn retired_command(app: &Router, auth: (&str, &str), path: &str, payload: &str) {
    for _ in 0..2 {
        let response = call(app, "POST", path, auth, payload, false).await;
        assert_eq!(response.status(), StatusCode::GONE);
        private(&response);
        let value: Value = serde_json::from_str(&text(response).await).unwrap();
        assert_eq!(value["error"]["code"], "COLLECTION_RETIRED");
    }
}

// Sync psql must not block the driver that rolls back a rejected C transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn audience_sources_http_retained_history_real_reading_and_retired_capture() {
    for name in [
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
        "TOR_ORGANIC_ATTRIBUTION_ENABLED",
        "TOR_FLOW_VISITS_ENABLED",
    ] {
        std::env::set_var(name, "1");
    }
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let report_store = PgDiscoveryAudience::new(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("SourcesHttpPassword2026").unwrap();
    let csrf = generate_session_token();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    for role in ["admin", "reader"] {
        let token = generate_session_token();
        users.push(
            accounts
                .register_with_session(
                    &format!("source-{role}-{stamp}@example.invalid"),
                    "Source fixture",
                    &password,
                    &session_token_digest(&token),
                    NonZeroU32::new(3600).unwrap(),
                )
                .await
                .unwrap(),
        );
        cookies.push(format!("tor-dev-session={token}; tor-dev-csrf={csrf}"));
    }
    let (admin, reader) = (users[0], users[1]);
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator');
         INSERT INTO rec_subjects(user_id,enabled,enabled_at,flow_epoch,flow_enabled_at)
         VALUES({reader},true,now()-interval '1 day',gen_random_uuid(),now()-interval '1 day')"
        ),
    );
    let scope = pg(
        &url,
        &format!("SELECT row_to_json(s)::text FROM rec_subjects s WHERE user_id={reader}"),
    );
    let mut books = Vec::new();
    for suffix in ["mixed", "direct", "later"] {
        books.push(pg(&url, &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES('ci:flow:sources:http:{stamp}:{suffix}','ru','SOURCE-PRIVATE-{stamp}-{suffix}','public','basic') RETURNING id"
        )).parse::<i64>().unwrap());
    }
    let mixed = books[0];
    let first = add_chapter(&url, mixed, 1);
    let second = add_chapter(&url, mixed, 2);
    let direct = add_chapter(&url, books[1], 1);
    let later = add_chapter(&url, books[2], 1);
    // Historical aggregate fixture only. There is no corresponding new
    // assignment, click, C attribution receipt or user-level analytics row.
    pg(
        &url,
        &format!(
            "INSERT INTO organic_discovery_quality_daily
         (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions) VALUES
         ((now() AT TIME ZONE 'UTC')::date,{mixed},'catalog',2,1,1,0,0),
         ((now() AT TIME ZONE 'UTC')::date,{mixed},'search',2,1,1,1,1),
         ((now() AT TIME ZONE 'UTC')::date,{},'unattributed',0,0,0,1,1),
         ((now() AT TIME ZONE 'UTC')::date-7,{mixed},'catalog',1,0,0,0,0);
         INSERT INTO rec_quality_daily
         (bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,
          impressions,paired_clicks,unpaired_clicks,starts,completions,book_opens) VALUES
         ((now() AT TIME ZONE 'UTC')::date,{mixed},'home','content','similar_by_content',1,
          'retained-http-fixture','recommendations_v1',2,1,0,0,0,1),
         ((now() AT TIME ZONE 'UTC')::date-7,{mixed},'home','content','similar_by_content',1,
          'retained-http-fixture','recommendations_v1',1,0,0,0,0,NULL)",
            books[1]
        ),
    );
    let baseline = report_store.report(7).await.unwrap();
    assert_eq!(optional_capture(&url, reader, &books), "0");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let admin_auth = (cookies[0].as_str(), csrf.as_str());
    let reader_auth = (cookies[1].as_str(), csrf.as_str());
    for (auth, status) in [
        (("", ""), StatusCode::UNAUTHORIZED),
        (reader_auth, StatusCode::FORBIDDEN),
    ] {
        let response = call(&app, "GET", "/admin/audience?days=invalid", auth, "", false).await;
        assert_eq!(response.status(), status);
        private(&response);
        assert!(!text(response).await.contains("data-metric="));
    }
    for path in [
        "/catalog?sort=newest".to_owned(),
        format!("/catalog?q=SOURCE-PRIVATE-{stamp}-mixed"),
    ] {
        let response = call(&app, "GET", &path, reader_auth, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = text(response).await;
        assert!(html.contains(&format!("href=\"/books/{mixed}\"")));
        assert!(!html.contains("data-organic-ticket") && !html.contains("data-rec-ticket"));
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/books/{mixed}"),
                reader_auth,
                "",
                false
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let feed = call(
        &app,
        "GET",
        "/api/v1/recommendations?surface=home",
        reader_auth,
        "",
        false,
    )
    .await;
    assert_eq!(feed.status(), StatusCode::OK);
    let feed: Value = serde_json::from_str(&text(feed).await).unwrap();
    assert_eq!(feed["personalized"], false);
    assert!(feed["generation_id"].is_null());
    assert!(feed["items"].as_array().unwrap().iter().all(|item| {
        item["source"] == "catalog"
            && item["reason"] == "new_release"
            && item.get("ticket").is_none()
    }));
    let book_id = mixed.to_string();
    assert!(feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["book_id"].as_str() == Some(book_id.as_str())));
    let ticket = generate_session_token();
    for path in [
        "/api/v1/recommendations/activate",
        "/api/v1/discovery/activate",
    ] {
        retired_command(
            &app,
            reader_auth,
            path,
            &format!("ticket={ticket}&csrf_token={csrf}"),
        )
        .await;
    }
    for path in [
        "/me/recommendations/consent",
        "/me/recommendations/flow-consent",
    ] {
        retired_command(
            &app,
            reader_auth,
            path,
            &format!(
                "enabled=1&expected_revision=0&command_key={}&csrf_token={csrf}",
                generate_session_token()
            ),
        )
        .await;
    }
    complete(&app, reader_auth, first).await;
    complete(&app, reader_auth, second).await;
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/books/{}", books[1]),
            reader_auth,
            "",
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
    complete(&app, reader_auth, direct).await;
    let event = pg(&url, &format!(
        "SELECT source_event_id FROM reader_chapter_completions WHERE user_id={reader} AND chapter_id={}",
        first.0
    ));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM reader_chapter_completions WHERE user_id={reader}")
        ),
        "3"
    );
    assert_eq!(optional_capture(&url, reader, &books), "0");
    assert_eq!(
        report_store.report(7).await.unwrap().current,
        baseline.current
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={mixed}"),
    );
    for days in [1, 7, 30, 90] {
        let response = call(
            &app,
            "GET",
            &format!("/admin/audience?days={days}"),
            admin_auth,
            "",
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        private(&response);
        let html = text(response).await;
        assert_report(&html, &report_store.report(days).await.unwrap());
        assert!(html.contains("data-testid=\"collection-retired\""));
        assert!(html.contains("data-current-measurement=\"unavailable\""));
        assert!(html.contains("data-measurement=\"legacy-consented\""));
        for forbidden in [
            "SOURCE-PRIVATE-",
            "Readable text",
            "source-reader-",
            event.as_str(),
            ticket.as_str(),
        ] {
            assert!(
                !html.contains(forbidden),
                "private source or content leaked"
            );
        }
    }
    assert_eq!(
        report_store.report(7).await.unwrap().current,
        baseline.current,
        "later reading, report GETs and a book hold do not alter retained aggregate evidence"
    );
    complete(&app, reader_auth, later).await;
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM reader_chapter_completions WHERE user_id={reader}")
        ),
        "4"
    );
    assert_eq!(optional_capture(&url, reader, &books), "0");
    assert_eq!(
        pg(
            &url,
            &format!("SELECT row_to_json(s)::text FROM rec_subjects s WHERE user_id={reader}")
        ),
        scope,
        "retired commands must not toggle or rotate an old scope"
    );
    assert_eq!(
        report_store.report(7).await.unwrap().current,
        baseline.current
    );
    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    let denied = call(&app, "GET", "/admin/audience", admin_auth, "", false).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    private(&denied);
    assert!(!text(denied).await.contains("data-metric="));
    let ids = books
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    pg(&url, &format!(
        "BEGIN;
         DELETE FROM account_roles WHERE user_id={admin};
         DELETE FROM reader_book_positions WHERE book_id IN({ids});
         DELETE FROM reader_chapter_starts WHERE book_id IN({ids});
         DELETE FROM reader_chapter_completions WHERE book_id IN({ids});
         DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id IN({ids}));
         DELETE FROM book_members WHERE book_id IN({ids});
         DELETE FROM books WHERE id IN({ids});
         DELETE FROM rec_quality_daily WHERE book_id IN({ids});
         DELETE FROM organic_discovery_quality_daily WHERE book_id IN({ids});
         DELETE FROM rec_dirty_buckets WHERE book_id IN({ids});
         DELETE FROM organic_discovery_dirty WHERE book_id IN({ids});
         DELETE FROM users WHERE id IN({admin},{reader});
         COMMIT"
    ));
}
