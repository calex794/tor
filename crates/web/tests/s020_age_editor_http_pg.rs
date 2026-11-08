//! S020 P0: draft HTTP age settings accept only live scoped, monotonic Book tiers.
//! Mutations are off by default; this fixture uses disposable PostgreSQL tor_ci.
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

fn pg(url: &str, query: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let result = Command::new("psql")
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
            query,
        ])
        .output()
        .expect("postgres client");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn get(app: &Router, path: &str, cookie: &str) -> StatusCode {
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    reply.status()
}

async fn page(app: &Router, path: &str, cookie: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn save_to_library(app: &Router, book: i64, cookie: &str, csrf: &str) -> StatusCode {
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/me/library/{book}/save"))
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    reply.status()
}

async fn library_has_book(app: &Router, cookie: &str, book: i64) -> bool {
    let (status, body) = page(app, "/api/v1/me/library", cookie).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    json["items"].as_array().unwrap().iter().any(|item| {
        item["book_id"]
            .as_str()
            .and_then(|raw| raw.parse::<i64>().ok())
            == Some(book)
    })
}

async fn submit(
    app: &Router,
    path: &str,
    cookie: &str,
    origin: &str,
    csrf: &str,
    expected: &str,
    requested: &str,
) -> StatusCode {
    let encoded =
        format!("csrf_token={csrf}&expected_level={expected}&requested_level={requested}");
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(encoded))
                .unwrap(),
        )
        .await
        .unwrap();
    reply.status()
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL /tor_ci"]
async fn s020_age_editor_is_off_by_default_and_monotonic_under_live_http() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let marker = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let owner_token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let password = hash_password("S020AgeEditorCI2026!").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("s020-age-owner-{marker}@example.invalid"),
            "S020 age owner",
            &password,
            &session_token_digest(&owner_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let outsider = accounts
        .register_with_session(
            &format!("s020-age-stranger-{marker}@example.invalid"),
            "S020 age stranger",
            &password,
            &session_token_digest(&outsider_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,owner_user_id,visibility,access_level)
             VALUES('ci:s020:age:book:{marker}','ru','S020 guarded Book', {owner}, 'public', 'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    // The same published free text, TOC metadata and paid preview must all
    // react to the live Book tier changed by this exact settings POST.
    let free: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'S020 free chapter','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let paid: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},2,'S020 paid preview','public','paid',100) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let free_marker = format!("S020-AGE-FREE-TEXT-{marker}");
    let paid_marker = format!("S020-AGE-PAID-SECRET-{marker}");
    for (chapter, marker_text) in [(free, free_marker.as_str()), (paid, paid_marker.as_str())] {
        let version: i64 = pg(
            &url,
            &format!(
                "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
                 VALUES({chapter},1,'{marker_text}') RETURNING id"
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
    }
    let free_path = format!("/reader/{free}");
    let paid_path = format!("/reader/{paid}");
    let toc_path = format!("/api/v1/books/{book}/toc");
    let path = format!("/me/books/{book}/settings/access-level");
    let library_prompt = format!("/me/library/add/{book}");
    let discussion = format!("/books/{book}/comments");
    let room = format!("/books/{book}/room");
    let public = format!("/books/{book}");
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let stranger_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");

    std::env::remove_var("TOR_S020_ACCESS_EDITOR_ENABLED");
    let disabled = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    assert_eq!(
        get(&disabled, &path, &owner_cookie).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        submit(
            &disabled,
            &path,
            &owner_cookie,
            ORIGIN,
            &csrf,
            "basic",
            "adult"
        )
        .await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT access_level FROM books WHERE id={book}")
        ),
        "basic",
    );

    // Tests opt into the route in this disposable process only; no production
    // deployment or persistent DB migration is performed.
    std::env::set_var("TOR_S020_ACCESS_EDITOR_ENABLED", "1");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    std::env::remove_var("TOR_S020_ACCESS_EDITOR_ENABLED");

    assert_eq!(get(&app, &path, "").await, StatusCode::SEE_OTHER);
    assert_eq!(get(&app, &path, &owner_cookie).await, StatusCode::OK);
    assert_eq!(
        get(&app, &path, &stranger_cookie).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&app, &public, "").await, StatusCode::OK);
    assert_eq!(get(&app, &toc_path, "").await, StatusCode::OK);
    // An authorized reader must be able to follow the Book page's
    // "Add to library" link; this personal prompt is never anonymous data.
    assert_eq!(get(&app, &library_prompt, "").await, StatusCode::SEE_OTHER);
    for candidate in [&library_prompt, &discussion, &room] {
        assert_eq!(get(&app, candidate, &stranger_cookie).await, StatusCode::OK);
    }
    assert_eq!(
        save_to_library(&app, book, &stranger_cookie, &csrf).await,
        StatusCode::SEE_OTHER
    );
    assert!(library_has_book(&app, &stranger_cookie, book).await);
    let (free_status, free_html) = page(&app, &free_path, "").await;
    assert_eq!(free_status, StatusCode::OK);
    assert!(free_html.contains(&free_marker));
    let (paid_status, paid_html) = page(&app, &paid_path, "").await;
    assert_eq!(paid_status, StatusCode::OK);
    assert!(
        !paid_html.contains(&paid_marker),
        "paid plaintext must not leak"
    );
    assert!(paid_html.contains("S020 paid preview"));
    assert_eq!(
        get(&app, &free_path, &stranger_cookie).await,
        StatusCode::OK
    );
    assert_eq!(
        submit(
            &app,
            &path,
            &stranger_cookie,
            ORIGIN,
            &csrf,
            "basic",
            "adult"
        )
        .await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        submit(
            &app,
            &path,
            &owner_cookie,
            "https://wrong.invalid",
            &csrf,
            "basic",
            "adult"
        )
        .await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        submit(
            &app,
            &path,
            &owner_cookie,
            ORIGIN,
            "forged",
            "basic",
            "adult"
        )
        .await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, &csrf, "basic", "adult").await,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT access_level FROM books WHERE id={book}")
        ),
        "adult",
    );
    assert_eq!(get(&app, &public, "").await, StatusCode::NOT_FOUND);
    for candidate in [&public, &toc_path, &free_path, &paid_path] {
        assert_eq!(get(&app, candidate, "").await, StatusCode::NOT_FOUND);
        assert_eq!(
            get(&app, candidate, &stranger_cookie).await,
            StatusCode::NOT_FOUND
        );
    }
    // Revocation is live across private bookmarks and social metadata,
    // not just the public reader or the paid preview.
    for candidate in [&library_prompt, &discussion, &room] {
        assert_eq!(
            get(&app, candidate, &stranger_cookie).await,
            StatusCode::NOT_FOUND
        );
    }
    assert!(!library_has_book(&app, &stranger_cookie, book).await);
    let (denied, denied_html) = page(&app, &free_path, "").await;
    assert_eq!(denied, StatusCode::NOT_FOUND);
    assert!(!denied_html.contains(&free_marker));
    pg(
        &url,
        &format!("UPDATE users SET adult_enabled=TRUE WHERE id={outsider}"),
    );
    for candidate in [&public, &toc_path, &free_path, &paid_path] {
        assert_eq!(get(&app, candidate, &stranger_cookie).await, StatusCode::OK);
    }
    assert!(page(&app, &free_path, &stranger_cookie)
        .await
        .1
        .contains(&free_marker));
    assert!(!page(&app, &paid_path, &stranger_cookie)
        .await
        .1
        .contains(&paid_marker));
    for candidate in [&library_prompt, &discussion, &room] {
        assert_eq!(get(&app, candidate, &stranger_cookie).await, StatusCode::OK);
    }
    assert!(library_has_book(&app, &stranger_cookie, book).await);
    assert_eq!(
        submit(
            &app,
            &path,
            &owner_cookie,
            ORIGIN,
            &csrf,
            "basic",
            "pro_adult"
        )
        .await,
        StatusCode::CONFLICT,
    );
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, &csrf, "adult", "basic").await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        submit(
            &app,
            &path,
            &owner_cookie,
            ORIGIN,
            &csrf,
            "adult",
            "pro_adult"
        )
        .await,
        StatusCode::NOT_FOUND,
    );
    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({owner},'pro_adult','moderation',TRUE)"
        ),
    );
    assert_eq!(
        submit(
            &app,
            &path,
            &owner_cookie,
            ORIGIN,
            &csrf,
            "adult",
            "pro_adult"
        )
        .await,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT access_level FROM books WHERE id={book}")
        ),
        "pro_adult",
    );
    // An 18+ account is not automatically entitled to 21+ chapter text,
    // public metadata, paid previews, TOC or its old saved bookmark.
    for candidate in [&library_prompt, &discussion, &room] {
        assert_eq!(
            get(&app, candidate, &stranger_cookie).await,
            StatusCode::NOT_FOUND
        );
    }
    assert!(!library_has_book(&app, &stranger_cookie, book).await);
    for candidate in [&public, &toc_path, &free_path, &paid_path] {
        assert_eq!(
            get(&app, candidate, &stranger_cookie).await,
            StatusCode::NOT_FOUND
        );
    }
    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({outsider},'pro_adult','moderation',TRUE)"
        ),
    );
    for candidate in [&public, &toc_path, &free_path, &paid_path] {
        assert_eq!(get(&app, candidate, &stranger_cookie).await, StatusCode::OK);
    }
    assert!(page(&app, &free_path, &stranger_cookie)
        .await
        .1
        .contains(&free_marker));
    assert!(!page(&app, &paid_path, &stranger_cookie)
        .await
        .1
        .contains(&paid_marker));
    for candidate in [&library_prompt, &discussion, &room] {
        assert_eq!(get(&app, candidate, &stranger_cookie).await, StatusCode::OK);
    }
    assert!(library_has_book(&app, &stranger_cookie, book).await);
    pg(
        &url,
        &format!(
            "UPDATE user_level_grants SET is_active=FALSE
             WHERE user_id={outsider} AND level='pro_adult'"
        ),
    );
    for candidate in [&public, &toc_path, &free_path, &paid_path] {
        assert_eq!(
            get(&app, candidate, &stranger_cookie).await,
            StatusCode::NOT_FOUND
        );
    }
    assert!(!page(&app, &free_path, &stranger_cookie)
        .await
        .1
        .contains(&free_marker));
    for candidate in [&library_prompt, &discussion, &room] {
        assert_eq!(
            get(&app, candidate, &stranger_cookie).await,
            StatusCode::NOT_FOUND
        );
    }
    assert!(!library_has_book(&app, &stranger_cookie, book).await);
    // Revocation closes reader access; the signed-in Book settings permission
    // is independent of reader age eligibility.
    pg(
        &url,
        &format!("UPDATE user_level_grants SET is_active=FALSE WHERE user_id={owner}"),
    );
    assert_eq!(
        get(&app, &public, &owner_cookie).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&app, &path, &owner_cookie).await, StatusCode::OK);

    // Deletion cannot be edited through the ordinary settings route.
    pg(
        &url,
        &format!("UPDATE books SET lifecycle_status='pending_deletion' WHERE id={book}"),
    );
    assert_eq!(get(&app, &path, &owner_cookie).await, StatusCode::NOT_FOUND);
    assert_eq!(
        submit(
            &app,
            &path,
            &owner_cookie,
            ORIGIN,
            &csrf,
            "pro_adult",
            "pro_adult"
        )
        .await,
        StatusCode::NOT_FOUND,
    );
    // A finalized ordinary deletion is not merely pending: the Book still
    // exists for historical library editions but its age settings are frozen.
    pg(
        &url,
        &format!(
            "UPDATE books SET lifecycle_status='ordinary_deleted',
             ordinary_deleted_at=now(), visibility='draft' WHERE id={book}"
        ),
    );
    assert_eq!(get(&app, &path, &owner_cookie).await, StatusCode::NOT_FOUND);
    assert_eq!(
        submit(
            &app,
            &path,
            &owner_cookie,
            ORIGIN,
            &csrf,
            "pro_adult",
            "pro_adult"
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT access_level FROM books WHERE id={book}")
        ),
        "pro_adult"
    );
    pool.close().await;
}
