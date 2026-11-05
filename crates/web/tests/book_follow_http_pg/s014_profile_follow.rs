//! S014/E-05: independent profile follow through live session and PostgreSQL.
use super::*;

async fn s014_profile_page(app: &Router, name: &str, cookies: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/users/{name}"))
                .header(header::COOKIE, cookies)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

async fn s014_creator_list(app: &Router, cookies: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/me/creators")
                .header(header::COOKIE, cookies)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn e_05_s014_profile_follow_is_account_scoped_and_keeps_period_history() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "no permanent database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("ProfileFollowTest2026").unwrap();
    let author_token = generate_session_token();
    let reader_token = generate_session_token();
    let inactive_token = generate_session_token();
    let csrf = generate_session_token();
    let author_name = format!("S014Writer{nonce}");
    let reader_name = format!("S014Reader{nonce}");
    let inactive_name = format!("S014Dormant{nonce}");
    let author = accounts
        .register_with_session(
            &format!("s014-author-{nonce}@example.invalid"),
            &author_name,
            &password,
            &session_token_digest(&author_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let reader = accounts
        .register_with_session(
            &format!("s014-reader-{nonce}@example.invalid"),
            &reader_name,
            &password,
            &session_token_digest(&reader_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let inactive = accounts
        .register_with_session(
            &format!("s014-dormant-{nonce}@example.invalid"),
            &inactive_name,
            &password,
            &session_token_digest(&inactive_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();

    // Profile follow must work even when the author's only Book is unlisted
    // and age-restricted; this does not grant reading rights.
    pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level,lifecycle_status)
             VALUES('ci:s014:hidden:{nonce}','ru','Unlisted author Book','public',{author},'pro_adult','pending_deletion')"
        ),
    );
    pg(
        &url,
        &format!("UPDATE users SET account_state='suspended' WHERE id={inactive}"),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let reader_cookies = format!("tor-dev-session={reader_token}; tor-dev-csrf={csrf}");
    let author_cookies = format!("tor-dev-session={author_token}; tor-dev-csrf={csrf}");
    let follow = format!("/users/{author}/subscribe");
    let unfollow = format!("/users/{author}/unsubscribe");

    let (guest_status, guest_profile) = s014_profile_page(&app, &author_name, "").await;
    assert_eq!(guest_status, StatusCode::OK);
    assert!(!guest_profile.contains(&format!("action=\"{follow}\"")));
    let (self_status, self_profile) = s014_profile_page(&app, &author_name, &author_cookies).await;
    assert_eq!(self_status, StatusCode::OK);
    assert!(!self_profile.contains(&format!("action=\"{follow}\"")));
    let (status, profile) = s014_profile_page(&app, &author_name, &reader_cookies).await;
    assert_eq!(status, StatusCode::OK);
    assert!(profile.contains(&format!("action=\"{follow}\"")));
    assert!(profile.contains(&format!("value=\"{csrf}\"")));
    assert_eq!(
        s014_profile_page(&app, &inactive_name, &reader_cookies)
            .await
            .0,
        StatusCode::NOT_FOUND,
        "suspended profiles are not public",
    );

    assert_eq!(
        post(
            &app,
            &follow,
            &reader_cookies,
            "https://wrong.invalid",
            &csrf
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&app, &follow, &reader_cookies, ORIGIN, "invalid").await,
        StatusCode::FORBIDDEN
    );
    assert!(matches!(
        post(&app, &follow, "", ORIGIN, &csrf).await,
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
    ));
    assert_eq!(
        post(
            &app,
            &format!("/users/{reader}/subscribe"),
            &reader_cookies,
            ORIGIN,
            &csrf
        )
        .await,
        StatusCode::BAD_REQUEST,
        "NEwBOO self-follow is invalid input, not an authorization error",
    );
    assert_eq!(
        post(
            &app,
            &format!("/users/{inactive}/subscribe"),
            &reader_cookies,
            ORIGIN,
            &csrf
        )
        .await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        post(
            &app,
            "/users/999999999999/subscribe",
            &reader_cookies,
            ORIGIN,
            &csrf
        )
        .await,
        StatusCode::NOT_FOUND,
    );

    assert_eq!(
        post(&app, &follow, &reader_cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    let first = pg(
        &url,
        &format!(
            "SELECT id::text || ':' || started_at::text FROM creator_follow_periods
             WHERE follower_user_id={reader} AND creator_user_id={author} AND ended_at IS NULL"
        ),
    );
    assert_eq!(
        pg(&url, &format!(
            "SELECT source_kind || ':' || COALESCE(source_book_id::text,'NULL')
             FROM creator_follow_periods WHERE follower_user_id={reader} AND creator_user_id={author} AND ended_at IS NULL"
        )),
        "profile:NULL",
    );
    assert_eq!(
        post(&app, &follow, &reader_cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT id::text || ':' || started_at::text FROM creator_follow_periods
             WHERE follower_user_id={reader} AND creator_user_id={author} AND ended_at IS NULL"
            )
        ),
        first,
        "repeated POST must not reset continuous following tenure",
    );
    assert!(s014_profile_page(&app, &author_name, &reader_cookies)
        .await
        .1
        .contains(&format!("action=\"{unfollow}\"")));
    assert!(s014_creator_list(&app, &reader_cookies)
        .await
        .contains(&author_name));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM reader_book_follows WHERE user_id={reader}")
        ),
        "0"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={reader}")
        ),
        "0"
    );

    // The existing creator-follow release view is source-kind neutral. A
    // new free release for a separate, visible Book reaches the user's feed.
    let visible_book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:s014:visible:{nonce}','ru','S014 new author work','public',{author},'basic') RETURNING id"
        ),
    ).parse().unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({visible_book},1,'S014 chapter from profile subscription','public','free',0) RETURNING id"
        ),
    ).parse().unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Reader text') RETURNING id"
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
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_first_release_events(chapter_id,version_id,creator_user_id)
         VALUES({chapter},{version},{author})"
        ),
    );
    assert!(followed_page(&app, &reader_cookies)
        .await
        .contains("S014 chapter from profile subscription"));

    assert_eq!(
        post(&app, &unfollow, &reader_cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post(&app, &unfollow, &reader_cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM creator_follow_periods
         WHERE follower_user_id={reader} AND creator_user_id={author} AND ended_at IS NOT NULL"
            )
        ),
        "1"
    );
    assert!(s014_profile_page(&app, &author_name, &reader_cookies)
        .await
        .1
        .contains(&format!("action=\"{follow}\"")));

    // Book-derived follow after unfollow starts a new period. A later
    // profile follow must not overwrite its original B-verified attribution.
    assert_eq!(
        post(
            &app,
            &format!("/me/books/{visible_book}/follow-creator"),
            &reader_cookies,
            ORIGIN,
            &csrf
        )
        .await,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        post(&app, &follow, &reader_cookies, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT source_kind || ':' || source_book_id::text
             FROM creator_follow_periods
             WHERE follower_user_id={reader} AND creator_user_id={author} AND ended_at IS NULL"
            )
        ),
        format!("book_page:{visible_book}"),
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM creator_follow_periods
         WHERE follower_user_id={reader} AND creator_user_id={author}"
            )
        ),
        "2"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={reader}")
        ),
        "0"
    );
    // Revoking the A session after follow must immediately disable further
    // profile mutations without destroying the already-owned follow period.
    pg(
        &url,
        &format!("UPDATE sessions SET revoked_at=now() WHERE user_id={reader}"),
    );
    assert_eq!(
        post(&app, &unfollow, &reader_cookies, ORIGIN, &csrf).await,
        StatusCode::UNAUTHORIZED,
        "stale signed cookie cannot mutate an existing author follow",
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM creator_follow_periods
                 WHERE follower_user_id={reader} AND creator_user_id={author} AND ended_at IS NULL"
            ),
        ),
        "1",
        "revoking the session never deletes an existing author subscription",
    );
    pool.close().await;
}
