//! S020/E-05: real HTTP creator subscribe and historical source title age gates.
use super::*;

async fn creator_page(app: &Router, cookies: &str) -> String {
    let request = Request::builder()
        .uri("/me/creators")
        .header(header::COOKIE, cookies)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    String::from_utf8(to_bytes(response.into_body(), 128 * 1024).await.unwrap().to_vec())
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn e_05_s020_creator_follow_http_age_matrix() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("E05AgeFollow2026").unwrap();
    let author_token = generate_session_token();
    let reader_token = generate_session_token();
    let csrf = generate_session_token();
    let author = accounts
        .register_with_session(
            &format!("e05-http-author-{nonce}@example.invalid"),
            "E05 author",
            &password,
            &session_token_digest(&author_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let reader = accounts
        .register_with_session(
            &format!("e05-http-reader-{nonce}@example.invalid"),
            "E05 reader",
            &password,
            &session_token_digest(&reader_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:e05:http:{nonce}','ru','E05 restricted source','public',{author},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let cookies = format!("tor-dev-session={reader_token}; tor-dev-csrf={csrf}");
    let follow = format!("/me/books/{book}/follow-creator");
    assert_eq!(post(&app, &follow, &cookies, ORIGIN, &csrf).await, StatusCode::SEE_OTHER);
    assert!(creator_page(&app, &cookies).await.contains("E05 restricted source"));
    assert_eq!(
        pg(&url, &format!(
            "SELECT count(*) FROM creator_follow_periods
             WHERE follower_user_id={reader} AND creator_user_id={author} AND ended_at IS NULL"
        )),
        "1",
    );

    pg(&url, &format!("UPDATE books SET access_level='adult' WHERE id={book}"));
    assert_eq!(post(&app, &follow, &cookies, ORIGIN, &csrf).await, StatusCode::NOT_FOUND);
    let denied = creator_page(&app, &cookies).await;
    assert!(!denied.contains("E05 restricted source"));
    assert!(denied.contains(&format!("/me/creators/{author}/unfollow")));
    pg(&url, &format!("UPDATE users SET adult_enabled=TRUE WHERE id={reader}"));
    assert_eq!(post(&app, &follow, &cookies, ORIGIN, &csrf).await, StatusCode::SEE_OTHER);
    assert!(creator_page(&app, &cookies).await.contains("E05 restricted source"));

    pg(&url, &format!("UPDATE books SET access_level='pro_adult' WHERE id={book}"));
    assert_eq!(post(&app, &follow, &cookies, ORIGIN, &csrf).await, StatusCode::NOT_FOUND);
    assert!(!creator_page(&app, &cookies).await.contains("E05 restricted source"));
    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({reader},'pro_adult','moderation',TRUE)
             ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE"
        ),
    );
    pg(&url, &format!("UPDATE users SET adult_enabled=FALSE WHERE id={reader}"));
    assert_eq!(post(&app, &follow, &cookies, ORIGIN, &csrf).await, StatusCode::SEE_OTHER);
    assert!(creator_page(&app, &cookies).await.contains("E05 restricted source"));
    pg(
        &url,
        &format!(
            "UPDATE user_level_grants SET is_active=FALSE
             WHERE user_id={reader} AND level='pro_adult'"
        ),
    );
    assert_eq!(post(&app, &follow, &cookies, ORIGIN, &csrf).await, StatusCode::NOT_FOUND);
    assert!(!creator_page(&app, &cookies).await.contains("E05 restricted source"));

    pg(&url, &format!("UPDATE books SET access_level=NULL WHERE id={book}"));
    assert_eq!(post(&app, &follow, &cookies, ORIGIN, &csrf).await, StatusCode::NOT_FOUND);
    assert!(!creator_page(&app, &cookies).await.contains("E05 restricted source"));
    assert_eq!(
        post(
            &app,
            &format!("/me/creators/{author}/unfollow"),
            &cookies,
            ORIGIN,
            &csrf,
        )
        .await,
        StatusCode::SEE_OTHER,
        "age revoke must not strand existing creator subscriptions",
    );
    assert!(!creator_page(&app, &cookies).await.contains("E05 author"));
    pool.close().await;
}
