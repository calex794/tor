//! RET-R03: real PG-backed owner/site statistics from verified reader demand.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
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
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let output = Command::new("psql")
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
        .expect("psql");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn get(app: &Router, path: &str, session: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn html(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn owner_and_administrator_share_scoped_continuation_data_without_reader_ids() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut actors = Vec::new();
    let mut tokens = Vec::new();
    for role in ["owner", "other", "administrator"] {
        let token = generate_session_token();
        let actor = accounts
            .register_with_session(
                &format!("stats-{role}-{unique}@example.invalid"),
                role,
                &hash_password("ContinuationStatsTestOnly2026").unwrap(),
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        actors.push(actor);
        tokens.push(format!("tor-dev-session={token}"));
    }
    let owner = actors[0];
    let stranger = actors[1];
    let admin = actors[2];
    pg(
        &url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator')"),
    );
    let book: i64=pg(&url,&format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES('ci:ret:stats:{unique}','ru','Спрос <test>','public',{owner},'basic') RETURNING id"
    )).parse().unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES({book},1,'Первая','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES({chapter},1,'One chapter') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    pg(&url,&format!(
        "INSERT INTO reader_chapter_completions(user_id,book_id,chapter_id,first_version_id,source_event_id)
         VALUES({stranger},{book},{chapter},{version},gen_random_uuid())"
    ));
    pg(
        &url,
        &format!(
            "INSERT INTO reader_continuation_demand(user_id,book_id,frontier_chapter_id)
         VALUES({stranger},{book},{chapter}) ON CONFLICT DO NOTHING"
        ),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let owner_path = "/me/stats/continuations";
    let admin_path = "/admin/audience/continuations";
    assert_eq!(
        get(&app, owner_path, "").await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        get(&app, admin_path, "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, admin_path, &tokens[0]).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&app, owner_path, &tokens[1]).await.status(),
        StatusCode::OK
    );
    let page = get(&app, owner_path, &tokens[0]).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let page = html(page).await;
    assert!(page.contains("data-metric=\"waiting\">1"));
    assert!(page.contains("data-metric=\"new-week\">1"));
    assert!(page.contains("&lt;test&gt;"));
    assert!(!page.contains("stats-other-"));
    let admin_page = html(get(&app, admin_path, &tokens[2]).await).await;
    assert!(admin_page.contains("Спрос &lt;test&gt;"));
    // Sitewide totals can include unrelated fixtures from earlier PG tests.
    // Verify this exact book row without assuming a globally empty database.
    let row = admin_page
        .split(&format!(r#"href="/books/{book}""#))
        .nth(1)
        .expect("the book must appear in the administrator report")
        .split("</tr>")
        .next()
        .unwrap();
    assert!(row.contains("<td>1</td>"), "{row}");
    assert_eq!(
        get(&app, "/me/stats/continuations?after_book=2", &tokens[0])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(
            &app,
            "/admin/audience/continuations?unexpected=1",
            &tokens[2]
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    // A new published frontier changes current waiting but never deletes history.
    let next: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES({book},2,'Вторая','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let next_version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES({next},1,'Two chapters') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({next},{next_version})"
        ),
    );
    let after = html(get(&app, owner_path, &tokens[0]).await).await;
    assert!(after.contains("data-metric=\"waiting\">0"));
    assert!(after.contains("data-metric=\"historical\">1"));
    assert!(after.contains("<td>2</td><td>0</td>"));
    pool.close().await;
}
