//! F-12/S046: trusted C completion -> idempotent F grant -> private /me HTML.
//! Disposable PostgreSQL only; never contacts production or grants paid reading.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    net::SocketAddr,
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
use tor_recommendations::repositories::PgAchievements;
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
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn dashboard(app: &Router, cookie: &str) -> Response {
    let mut request = Request::builder()
        .uri("/me")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 49132))));
    app.clone().oneshot(request).await.unwrap()
}

async fn page(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s046_real_http_shows_only_c_verified_grants_to_authenticated_reader() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated test PostgreSQL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let token = generate_session_token();
    let foreign = generate_session_token();
    let hash = hash_password("S046TestPassword2026").unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor = accounts
        .register_with_session(
            &format!("s046-owner-{stamp}@example.invalid"),
            "BadgeOwner",
            &hash,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let other = accounts
        .register_with_session(
            &format!("s046-other-{stamp}@example.invalid"),
            "AnotherReader",
            &hash,
            &session_token_digest(&foreign),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let owner_cookie = format!("tor-dev-session={token}");
    let other_cookie = format!("tor-dev-session={foreign}");
    assert_eq!(dashboard(&app, "").await.status(), StatusCode::SEE_OTHER);
    let empty = dashboard(&app, &owner_cookie).await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(empty.headers()[header::CACHE_CONTROL], "no-store");
    assert!(page(empty).await.contains("Пока не получено: 0/1 глав"));

    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES('ci:s046:http:{stamp}','ru','S046 verified book','public','basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Test chapter','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'C-verified completion text') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{version})"
        ),
    );
    let event = pg(
        &url,
        &format!(
            "INSERT INTO reader_chapter_completions
               (user_id,book_id,chapter_id,first_version_id,source_event_id)
             VALUES({actor},{book},{chapter},{version},gen_random_uuid())
             RETURNING source_event_id::text"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO outbox_events
               (id,event_type,schema_version,aggregate_kind,aggregate_id,
                dedupe_key,payload,required_consumers)
             VALUES('{event}'::uuid,'ChapterCompletedV1',1,'chapter',{chapter},
               'ci:s046:http:completion:{stamp}',jsonb_build_object(
               'user_id',{actor},'book_id',{book},'chapter_id',{chapter},
               'version_id',{version},'authority','C-verified-free-position'),
               ARRAY['achievements-v1'])"
        ),
    );

    let achievements = PgAchievements::new(pool.clone());
    let (left, right) = tokio::join!(
        achievements.accept_completed_chapter(&event),
        achievements.accept_completed_chapter(&event)
    );
    assert_eq!(u8::from(left.unwrap()) + u8::from(right.unwrap()), 1);
    let granted: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM user_achievement_grants WHERE user_id={actor}"),
    )
    .parse()
    .unwrap();
    assert_eq!(granted, 1, "replay or race must not double-grant a badge");
    assert_eq!(
        achievements
            .snapshot(other)
            .await
            .unwrap()
            .completed_chapters,
        0
    );
    let private = dashboard(&app, &owner_cookie).await;
    assert_eq!(private.status(), StatusCode::OK);
    assert_eq!(private.headers()[header::CACHE_CONTROL], "no-store");
    let own_html = page(private).await;
    assert!(own_html.contains("Первая глава"));
    assert!(own_html.contains("Получено: 1/1 глав"));
    assert!(own_html.contains("Дата получения:"));
    assert!(
        !own_html.contains(&event),
        "no source event IDs in the page"
    );
    assert!(!own_html.contains("C-verified completion text"));
    let foreign_html = page(dashboard(&app, &other_cookie).await).await;
    assert!(!foreign_html.contains("Получено: 1/1 глав"));
    assert!(foreign_html.contains("Пока не получено: 0/1 глав"));
    assert!(!foreign_html.contains(&event));
}
