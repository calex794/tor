//! AB-08/09: real HTTP quota for public book comments, not login attempts.
use super::*;

#[tokio::test]
#[ignore = "requires isolated migrated PostgreSQL tor_ci"]
async fn e_02_scoped_comment_limit_preserves_csrf_replay_and_other_actor() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test DB URL");
    assert!(url.contains("127.0.0.1") && url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let session = generate_session_token();
    let csrf = generate_session_token();
    let actor = accounts
        .register_with_session(
            &format!("ab08-http-{nonce}@example.invalid"),
            "AB08 Writer",
            &hash_password("AB08DemoOnlyPassword2026!").unwrap(),
            &session_token_digest(&session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:ab08:{nonce}','ru','AB08 limit','public') RETURNING id"
        ),
    )
    .await
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let path = format!("/books/{book}/comments");
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let mut first_form = String::new();
    for i in 0..30 {
        let key = generate_session_token();
        let form = format!("body=AB08%20comment%20{i}&command_key={key}&csrf_token={csrf}");
        let response = call(&app, "POST", &path, &cookie, ORIGIN, &form).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "comment {i}");
        if i == 0 {
            first_form = form;
        }
    }

    let blocked_key = generate_session_token();
    let blocked_form =
        format!("body=The%2031st%20comment&command_key={blocked_key}&csrf_token={csrf}");
    let blocked = call(&app, "POST", &path, &cookie, ORIGIN, &blocked_form).await;
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u32 = blocked.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry));
    assert!(blocked.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM discussion_comments
                 WHERE author_user_id={actor} AND book_id={book}"
            ),
        )
        .await,
        "30"
    );

    // A resend of the same command is still an idempotent redirect; it does
    // not burn another slot or write a second comment after the 30th hit.
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, &first_form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    // Reusing an admitted key with a changed payload is not permission to
    // create or retarget a second comment.
    let conflicting = first_form.replace("AB08%20comment%200", "Changed%20payload");
    assert_ne!(conflicting, first_form);
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, &conflicting)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM discussion_comments
                 WHERE author_user_id={actor} AND book_id={book}"
            ),
        )
        .await,
        "30"
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            &cookie,
            "https://evil.invalid",
            &blocked_form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            &cookie,
            ORIGIN,
            &blocked_form.replace(&csrf, "forged"),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let other_session = generate_session_token();
    let other = accounts
        .register_with_session(
            &format!("ab08-http-other-{nonce}@example.invalid"),
            "Independent Writer",
            &hash_password("AB08AnotherDemoOnly2026!").unwrap(),
            &session_token_digest(&other_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let other_form = format!(
        "body=Independent&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    let other_cookie = format!("tor-dev-session={other_session}; tor-dev-csrf={csrf}");
    assert_eq!(
        call(&app, "POST", &path, &other_cookie, ORIGIN, &other_form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM discussion_comments WHERE book_id={book}")
        )
        .await,
        "31"
    );

    pg(
        &url,
        &format!("DELETE FROM discussion_comments WHERE book_id={book}"),
    )
    .await;
    pg(
        &url,
        &format!("DELETE FROM users WHERE id IN ({actor},{other})"),
    )
    .await;
    pg(&url, &format!("DELETE FROM books WHERE id={book}")).await;
}
