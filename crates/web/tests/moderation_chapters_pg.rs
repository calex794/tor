//! A-05/MOD-C01: chapter holds preserve author publication and paid ownership.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_contracts::auth::{AccountRole, UserId};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let output = std::process::Command::new("psql")
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
        .expect("psql installed");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    body: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn moderator_can_hold_and_release_free_and_paid_chapters_without_publishing() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("ModerationChapterHoldTestPassword2026!").unwrap();
    let ttl = NonZeroU32::new(3600).unwrap();
    let admin_token = generate_session_token();
    let moderator_token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let admin = accounts
        .register_with_session(
            &format!("mod-chapters-admin-{nonce}@example.invalid"),
            "Admin",
            &password,
            &session_token_digest(&admin_token),
            ttl,
        )
        .await
        .unwrap();
    let moderator = accounts
        .register_with_session(
            &format!("mod-chapters-moderator-{nonce}@example.invalid"),
            "Moderator",
            &password,
            &session_token_digest(&moderator_token),
            ttl,
        )
        .await
        .unwrap();
    accounts
        .register_with_session(
            &format!("mod-chapters-outsider-{nonce}@example.invalid"),
            "Outsider",
            &password,
            &session_token_digest(&outsider_token),
            ttl,
        )
        .await
        .unwrap();
    // The fixture alone bootstraps one administrator, never a user endpoint.
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \
             VALUES({admin},'administrator',TRUE)"
        ),
    );
    accounts
        .change_account_role(
            &session_token_digest(&admin_token),
            UserId::new(moderator).unwrap(),
            AccountRole::Moderator,
            true,
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:moderation:book:{nonce}','ru',
             '<script>private moderation title</script>','public') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'<chapter>protected free body</chapter>','public','free',0)
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let free_version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Free chapter text - never show in moderator queue')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{free_version})"
        ),
    );
    let paid_chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},2,'Paid chapter metadata','public','paid',199)
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let paid_version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({paid_chapter},1,'Paid chapter body - never show in moderator queue')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({paid_chapter},{paid_version})"
        ),
    );
    let reader = PgReader::new(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader.clone(),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let path = "/moderation/chapters";
    let action = format!("/moderation/chapters/{chapter}/hold");
    let paid_action = format!("/moderation/chapters/{paid_chapter}/hold");
    assert!(reader
        .public_paid_chapter_preview(paid_chapter as u64)
        .await
        .unwrap()
        .is_some());
    let mod_cookie = format!("tor-dev-session={moderator_token}; tor-dev-csrf={csrf}");
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    let outsider_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");

    assert_eq!(
        request(&app, "GET", path, "", ORIGIN, "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", path, &outsider_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &action,
            &outsider_cookie,
            ORIGIN,
            &format!("csrf_token={csrf}&expected_hold=false&new_hold=true")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let page = request(&app, "GET", path, &mod_cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let content = html(page).await;
    assert!(content.contains("&lt;script&gt;private moderation title&lt;/script&gt;"));
    assert!(!content.contains("<script>private moderation title</script>"));
    assert!(!content.contains("body_markdown"));
    assert!(content.contains("&lt;chapter&gt;protected free body&lt;/chapter&gt;"));
    assert!(content.contains("Paid chapter metadata"));
    assert!(!content.contains("Paid chapter body - never show in moderator queue"));
    assert!(!content.contains("Free chapter text - never show in moderator queue"));
    assert_eq!(
        request(
            &app,
            "GET",
            "/moderation/chapters?before=-1",
            &mod_cookie,
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        request(
            &app,
            "GET",
            "/moderation/chapters?status=invalid",
            &mod_cookie,
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let form = format!("csrf_token={csrf}&expected_hold=false&new_hold=true");
    assert_eq!(
        request(
            &app,
            "POST",
            &action,
            &mod_cookie,
            "https://evil.invalid",
            &form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &action,
            &format!("tor-dev-session={moderator_token}"),
            ORIGIN,
            &form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let applied = request(&app, "POST", &action, &mod_cookie, ORIGIN, &form).await;
    assert_eq!(applied.status(), StatusCode::SEE_OTHER);
    let current = pg(
        &url,
        &format!(
            "SELECT c.visibility || ':' || c.admin_hold::int || ':' || c.access_level
                || ':' || c.price_amount || ':' || p.version_id
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.id={chapter}"
        ),
    );
    assert_eq!(current, format!("public:1:free:0:{free_version}"));
    assert_eq!(
        request(&app, "GET", &format!("/books/{book}"), "", ORIGIN, "")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", &format!("/reader/{chapter}"), "", ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let filtered = request(
        &app,
        "GET",
        "/moderation/chapters?status=hold",
        &mod_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(filtered.status(), StatusCode::OK);
    assert!(html(filtered).await.contains("protected free body"));
    assert_eq!(
        request(&app, "POST", &paid_action, &mod_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let paid_current = pg(
        &url,
        &format!(
            "SELECT c.visibility || ':' || c.admin_hold::int || ':' || c.access_level
                || ':' || c.price_amount || ':' || p.version_id
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.id={paid_chapter}"
        ),
    );
    assert_eq!(paid_current, format!("public:1:paid:199:{paid_version}"));
    assert!(reader
        .public_paid_chapter_preview(paid_chapter as u64)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        request(&app, "POST", &action, &mod_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let audit_count: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM moderation_chapter_hold_events WHERE chapter_id={chapter}"),
    )
    .parse()
    .unwrap();
    assert_eq!(audit_count, 1, "stale submit must not duplicate audit");

    accounts
        .change_account_role(
            &session_token_digest(&admin_token),
            UserId::new(moderator).unwrap(),
            AccountRole::Moderator,
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        request(&app, "GET", path, &mod_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let release = format!("csrf_token={csrf}&expected_hold=true&new_hold=false");
    assert_eq!(
        request(&app, "POST", &action, &mod_cookie, ORIGIN, &release)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", &action, &admin_cookie, ORIGIN, &release)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let current = pg(
        &url,
        &format!(
            "SELECT c.visibility || ':' || c.admin_hold::int || ':' || c.access_level
                || ':' || c.price_amount || ':' || p.version_id
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.id={chapter}"
        ),
    );
    assert_eq!(current, format!("public:0:free:0:{free_version}"));
    assert_eq!(
        request(&app, "POST", &paid_action, &admin_cookie, ORIGIN, &release)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let paid_current = pg(
        &url,
        &format!(
            "SELECT c.visibility || ':' || c.admin_hold::int || ':' || c.access_level
                || ':' || c.price_amount || ':' || p.version_id
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.id={paid_chapter}"
        ),
    );
    assert_eq!(paid_current, format!("public:0:paid:199:{paid_version}"));
    assert!(reader
        .public_paid_chapter_preview(paid_chapter as u64)
        .await
        .unwrap()
        .is_some());
    let events = pg(
        &url,
        &format!(
            "SELECT actor_user_id || ':' || before_hold::int || ':' || after_hold::int
             FROM moderation_chapter_hold_events WHERE chapter_id={chapter} ORDER BY id"
        ),
    );
    assert_eq!(events, format!("{moderator}:0:1\n{admin}:1:0"));
    // NEwBOO distinguishes hide and block. Both deny public paid preview,
    // but neither rewrites author visibility, edition, price or buyer rights.
    let hide_form = format!("csrf_token={csrf}&expected_state=none&next_state=hidden");
    assert_eq!(
        request(
            &app,
            "POST",
            &paid_action,
            &admin_cookie,
            ORIGIN,
            &hide_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let hidden = pg(
        &url,
        &format!(
            "SELECT c.moderation_state || ':' || c.admin_hold::int || ':' ||
                c.visibility || ':' || c.access_level || ':' ||
                c.price_amount || ':' || p.version_id
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.id={paid_chapter}"
        ),
    );
    assert_eq!(hidden, format!("hidden:1:public:paid:199:{paid_version}"));
    assert!(reader
        .public_paid_chapter_preview(paid_chapter as u64)
        .await
        .unwrap()
        .is_none());
    let hidden_page = request(
        &app,
        "GET",
        "/moderation/chapters?status=hidden",
        &admin_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(hidden_page.status(), StatusCode::OK);
    assert!(html(hidden_page).await.contains("Paid chapter metadata"));
    assert_eq!(
        request(
            &app,
            "POST",
            &paid_action,
            &admin_cookie,
            ORIGIN,
            &hide_form
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    // Old MOD-C01 boolean release cannot bypass a more specific decision.
    let legacy_release = format!("csrf_token={csrf}&expected_hold=true&new_hold=false");
    assert_eq!(
        request(
            &app,
            "POST",
            &paid_action,
            &admin_cookie,
            ORIGIN,
            &legacy_release
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT moderation_state || ':' || admin_hold::int
                 FROM chapters WHERE id={paid_chapter}"
            )
        ),
        "hidden:1"
    );
    let block_form = format!("csrf_token={csrf}&expected_state=hidden&next_state=blocked");
    assert_eq!(
        request(
            &app,
            "POST",
            &paid_action,
            &admin_cookie,
            ORIGIN,
            &block_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT moderation_state || ':' || admin_hold::int
                 FROM chapters WHERE id={paid_chapter}"
            )
        ),
        "blocked:1"
    );
    let blocked_page = request(
        &app,
        "GET",
        "/moderation/chapters?status=blocked",
        &admin_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(blocked_page.status(), StatusCode::OK);
    assert!(html(blocked_page).await.contains("Paid chapter metadata"));
    // Direct old-style boolean SQL cannot remove a categorical block.
    let bypass = sqlx::query("UPDATE chapters SET admin_hold=false WHERE id=$1")
        .bind(paid_chapter)
        .execute(&pool)
        .await;
    assert!(
        bypass.is_err(),
        "legacy boolean writes must not bypass a categorical block"
    );

    let invalid_form = format!("csrf_token={csrf}&expected_state=blocked&next_state=published");
    assert_eq!(
        request(
            &app,
            "POST",
            &paid_action,
            &admin_cookie,
            ORIGIN,
            &invalid_form
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let unblock_form = format!("csrf_token={csrf}&expected_state=blocked&next_state=none");
    assert_eq!(
        request(
            &app,
            "POST",
            &paid_action,
            &admin_cookie,
            ORIGIN,
            &unblock_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(reader
        .public_paid_chapter_preview(paid_chapter as u64)
        .await
        .unwrap()
        .is_some());
    let decisions = pg(
        &url,
        &format!(
            "SELECT actor_user_id || ':' || before_state || ':' || after_state
             FROM moderation_chapter_decision_events
             WHERE chapter_id={paid_chapter} ORDER BY id"
        ),
    );
    assert_eq!(
        decisions,
        format!(
            "{moderator}:none:hold\n{admin}:hold:none\n{admin}:none:hidden\n{admin}:hidden:blocked\n{admin}:blocked:none"
        )
    );
    // The old SQL seeding path remains supported for generic boolean holds.
    let legacy_chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},3,'Legacy generic hold','draft','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!("UPDATE chapters SET admin_hold=true WHERE id={legacy_chapter}"),
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT moderation_state || ':' || admin_hold::int
                 FROM chapters WHERE id={legacy_chapter}"
            )
        ),
        "hold:1"
    );
    pg(
        &url,
        &format!("UPDATE chapters SET admin_hold=false WHERE id={legacy_chapter}"),
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT moderation_state || ':' || admin_hold::int
                 FROM chapters WHERE id={legacy_chapter}"
            )
        ),
        "none:0"
    );
}
