//! FLOW: actual navigation/C commands reach the private F source report.
//! No synthetic positive analytics facts are inserted into PostgreSQL.
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
    repositories::{audience::PgDiscoveryAudience, PgRecommendations},
    services::audience::{DiscoveryChannel, DiscoveryReport, DiscoverySource},
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

fn attribute(tag: &str, name: &str) -> String {
    let marker = format!("{name}=\"");
    tag.split(marker.as_str())
        .nth(1)
        .expect("server-issued attribute")
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

fn organic_ticket(html: &str, book: i64) -> String {
    let href = format!("href=\"/books/{book}\"");
    let tag = html
        .split('<')
        .find(|tag| tag.contains(&href) && tag.contains("data-organic-ticket="))
        .expect("ticket on an actual rendered catalogue result");
    attribute(tag, "data-organic-ticket")
}

fn source_key(source: DiscoverySource) -> &'static str {
    match source {
        DiscoverySource::Catalog => "catalog",
        DiscoverySource::Search => "search",
        DiscoverySource::Recommendations => "recommendations",
        DiscoverySource::Unattributed => "unattributed",
    }
}

fn channel(report: &DiscoveryReport, source: DiscoverySource) -> &DiscoveryChannel {
    report
        .current
        .channels
        .iter()
        .find(|row| row.source == source)
        .unwrap()
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

async fn settle(store: &PgRecommendations, url: &str) {
    // Other mandatory PG cases may leave more than one worker batch behind.
    for _ in 0..128 {
        store.tick_feedback(true, true).await.unwrap();
        if store.pending_read_events().await.unwrap().is_empty()
            && pg(
                url,
                "SELECT NOT EXISTS(SELECT 1 FROM rec_dirty_buckets)
                         AND NOT EXISTS(SELECT 1 FROM organic_discovery_dirty)",
            ) == "t"
        {
            return;
        }
    }
    panic!("feedback worker did not drain the disposable database backlog");
}

async fn activate(app: &Router, auth: (&str, &str), ticket: &str, organic: bool, book: i64) {
    let endpoint = if organic {
        "/api/v1/discovery/activate"
    } else {
        "/api/v1/recommendations/activate"
    };
    for _ in 0..2 {
        let response = call(
            app,
            "POST",
            endpoint,
            auth,
            &format!("ticket={ticket}&csrf_token={}", auth.1),
            false,
        )
        .await;
        assert_eq!(
            response.status(),
            if organic {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::SEE_OTHER
            }
        );
        assert_eq!(
            call(app, "GET", &format!("/books/{book}"), auth, "", false)
                .await
                .status(),
            StatusCode::OK
        );
    }
}

// Sync psql must not block the driver that rolls back a rejected C transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn audience_sources_http_real_navigation_last_source_replay_opt_out_and_live_role() {
    for name in [
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
        "TOR_ORGANIC_ATTRIBUTION_ENABLED",
    ] {
        std::env::set_var(name, "1");
    }
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let store = PgRecommendations::new(pool.clone());
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
        &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator')"),
    );
    let prior_hold = pg(&url, "SELECT hold FROM rec_runtime_policy WHERE singleton");
    pg(
        &url,
        "UPDATE rec_runtime_policy SET hold=true WHERE singleton",
    );
    let mut books = Vec::new();
    for suffix in ["mixed", "direct", "disabled"] {
        books.push(pg(&url, &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES('ci:flow:sources:http:{stamp}:{suffix}','ru','SOURCE-PRIVATE-{stamp}-{suffix}','public','basic') RETURNING id"
        )).parse::<i64>().unwrap());
    }
    let mixed = books[0];
    let first = add_chapter(&url, mixed, 1);
    let second = add_chapter(&url, mixed, 2);
    let direct = add_chapter(&url, books[1], 1);
    let disabled = add_chapter(&url, books[2], 1);
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
    let consent = format!(
        "enabled=1&expected_revision=0&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/me/recommendations/consent",
            reader_auth,
            &consent,
            false
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    settle(&store, &url).await;
    let before = report_store.report(7).await.unwrap();
    let legacy_rec_opens = pg(
        &url,
        "SELECT EXISTS(SELECT 1 FROM rec_quality_daily WHERE book_opens IS NULL
         AND bucket_date >= (now() AT TIME ZONE 'UTC')::date-6
         AND bucket_date < (now() AT TIME ZONE 'UTC')::date+1)",
    ) == "t";

    let catalogue = call(&app, "GET", "/catalog?sort=newest", reader_auth, "", false).await;
    assert_eq!(catalogue.status(), StatusCode::OK);
    let catalog_ticket = organic_ticket(&text(catalogue).await, mixed);
    activate(&app, reader_auth, &catalog_ticket, true, mixed).await;
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
    let book_id = mixed.to_string();
    let rec_ticket = feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["book_id"].as_str() == Some(book_id.as_str()))
        .expect("real fallback result")["ticket"]
        .as_str()
        .unwrap();
    activate(&app, reader_auth, rec_ticket, false, mixed).await;
    let search = call(
        &app,
        "GET",
        &format!("/catalog?q=SOURCE-PRIVATE-{stamp}-mixed"),
        reader_auth,
        "",
        false,
    )
    .await;
    assert_eq!(search.status(), StatusCode::OK);
    let search_ticket = organic_ticket(&text(search).await, mixed);
    activate(&app, reader_auth, &search_ticket, true, mixed).await;
    complete(&app, reader_auth, first).await;
    let event = pg(&url, &format!(
        "SELECT source_event_id FROM reader_chapter_completions WHERE user_id={reader} AND chapter_id={}", first.0
    ));
    let (one, two) = tokio::join!(
        store.accept_read_event(&event),
        store.accept_read_event(&event)
    );
    assert_eq!(usize::from(one.unwrap()) + usize::from(two.unwrap()), 1);
    assert!(!store.accept_read_event(&event).await.unwrap());
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
    settle(&store, &url).await;
    let after = report_store.report(7).await.unwrap();
    for source in [
        DiscoverySource::Catalog,
        DiscoverySource::Search,
        DiscoverySource::Recommendations,
        DiscoverySource::Unattributed,
    ] {
        let old = channel(&before, source);
        let new = channel(&after, source);
        let read_delta = if matches!(
            source,
            DiscoverySource::Search | DiscoverySource::Unattributed
        ) {
            1
        } else {
            0
        };
        assert_eq!(
            new.starts.unwrap_or(0),
            old.starts.unwrap_or(0) + read_delta
        );
        assert_eq!(
            new.completions.unwrap_or(0),
            old.completions.unwrap_or(0) + read_delta
        );
        if source == DiscoverySource::Unattributed {
            assert_eq!(new.clicks, None);
            assert_eq!(new.book_opens, None);
        } else {
            assert_eq!(new.clicks, Some(old.clicks.unwrap_or(0) + 1));
            if source == DiscoverySource::Recommendations && legacy_rec_opens {
                assert_eq!(
                    new.book_opens, None,
                    "partial old REC measurement stays unknown"
                );
            } else {
                assert_eq!(new.book_opens, Some(old.book_opens.unwrap_or(0) + 1));
            }
        }
    }
    assert_eq!(pg(&url, &format!("SELECT count(*) FROM reader_chapter_completions WHERE user_id={reader} AND book_id={mixed}")), "2");
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_first_conversions WHERE book_id={mixed}")
        ),
        "0"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT sum(book_opens) FROM rec_quality_daily WHERE book_id={mixed}")
        ),
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM organic_discovery_unattributed_reads WHERE book_id={mixed}"
            )
        ),
        "0"
    );
    assert!(after.health.fresh && !after.health.rec_dirty && !after.health.organic_dirty);
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
        assert!(html.contains("audience-current") && html.contains("audience-sources-health"));
        for forbidden in [
            "SOURCE-PRIVATE-",
            "Readable text",
            "source-reader-",
            event.as_str(),
            catalog_ticket.as_str(),
            search_ticket.as_str(),
            rec_ticket,
        ] {
            assert!(
                !html.contains(forbidden),
                "private source or content leaked"
            );
        }
    }
    assert_eq!(
        report_store.report(7).await.unwrap().current,
        after.current,
        "repeated aggregate GETs and a later book hold must not change past evidence"
    );

    let opt_out = format!(
        "expected_revision=1&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/me/recommendations/consent",
            reader_auth,
            &opt_out,
            false
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    complete(&app, reader_auth, disabled).await;
    settle(&store, &url).await;
    let erased = report_store.report(7).await.unwrap();
    for source in [
        DiscoverySource::Catalog,
        DiscoverySource::Search,
        DiscoverySource::Recommendations,
        DiscoverySource::Unattributed,
    ] {
        let old = channel(&before, source);
        let new = channel(&erased, source);
        assert_eq!(new.starts.unwrap_or(0), old.starts.unwrap_or(0));
        assert_eq!(new.completions.unwrap_or(0), old.completions.unwrap_or(0));
        assert_eq!(new.clicks.unwrap_or(0), old.clicks.unwrap_or(0));
    }
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM reader_chapter_completions WHERE user_id={reader}")
        ),
        "4"
    );
    let response = call(&app, "GET", "/admin/audience?days=7", admin_auth, "", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_report(&text(response).await, &erased);
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
         UPDATE rec_runtime_policy SET hold={} WHERE singleton;
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
         COMMIT", if prior_hold == "t" { "true" } else { "false" }
    ));
}
