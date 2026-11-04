//! S020/C-15: following a Book by its numeric ID requires live age rights.
//! No production database is used; CI runs against disposable tor_ci only.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn c_15_s020_book_follow_http_live_age_matrix() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("C15AgeFollow2026").unwrap();
    let token = generate_session_token();
    let csrf = generate_session_token();
    let reader = accounts
        .register_with_session(
            &format!("c15-age-follow-{nonce}@example.invalid"),
            "C15 reader",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES('ci:c15:age:{nonce}','ru','C15 age protected','public','basic') RETURNING id"
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
    let cookies = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let follow = format!("/me/books/{book}/follow");
    let unfollow = format!("/me/books/{book}/unfollow");

    assert_eq!(
        post(&app, &follow, &cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    pg(&url, &format!("UPDATE books SET access_level='adult' WHERE id={book}"));
    assert_eq!(
        post(&app, &follow, &cookies, ORIGIN, &csrf).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(&url, &format!(
            "SELECT count(*) FROM reader_book_follows WHERE book_id={book} AND user_id={reader}"
        )),
        "1",
        "age revocation must not delete existing follow"
    );
    assert_eq!(
        post(&app, &unfollow, &cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post(&app, &follow, &cookies, ORIGIN, &csrf).await,
        StatusCode::NOT_FOUND
    );

    pg(&url, &format!("UPDATE users SET adult_enabled=TRUE WHERE id={reader}"));
    assert_eq!(
        post(&app, &follow, &cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    pg(&url, &format!("UPDATE books SET access_level='pro_adult' WHERE id={book}"));
    assert_eq!(
        post(&app, &follow, &cookies, ORIGIN, &csrf).await,
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({reader},'pro_adult','moderation',TRUE)
             ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE"
        ),
    );
    pg(&url, &format!("UPDATE users SET adult_enabled=FALSE WHERE id={reader}"));
    assert_eq!(
        post(&app, &follow, &cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    pg(&url, &format!(
        "UPDATE user_level_grants SET is_active=FALSE WHERE user_id={reader} AND level='pro_adult'"
    ));
    assert_eq!(
        post(&app, &follow, &cookies, ORIGIN, &csrf).await,
        StatusCode::NOT_FOUND
    );
    pg(&url, &format!("UPDATE books SET access_level=NULL WHERE id={book}"));
    assert_eq!(
        post(&app, &follow, &cookies, ORIGIN, &csrf).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(&app, &unfollow, &cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    pool.close().await;
}
