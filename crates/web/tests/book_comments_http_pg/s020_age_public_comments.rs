//! S020/P0 #486: public Book comments and mentions must use live A/B rights.
use super::*;
use tor_discussions::repositories::public_book_comments::{CreateResult, PgPublicBookComments};

#[tokio::test]
#[ignore = "requires isolated migrated PostgreSQL tor_ci"]
async fn s020_public_comment_and_mention_age_rights_rechecked_on_every_request() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated test PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "do not use production");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let session = generate_session_token();
    let second_session = generate_session_token();
    let csrf = generate_session_token();
    let writer = accounts
        .register_with_session(
            &format!("s020-comment-writer-{nonce}@tor-ci.example"),
            "S020 Writer",
            &hash_password("S020AgeComments2026!").unwrap(),
            &session_token_digest(&session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let second = accounts
        .register_with_session(
            &format!("s020-comment-second-{nonce}@tor-ci.example"),
            "Second Reader",
            &hash_password("S020OtherComments2026!").unwrap(),
            &session_token_digest(&second_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,
                               visibility_status,access_level)
             VALUES('ci:s020:age-comments:{nonce}','ru','S020 gated title',
                    'public','active','basic') RETURNING id"
        ),
    )
    .await
    .parse()
    .unwrap();
    let repo = PgPublicBookComments::new(pool.clone());
    assert!(matches!(
        repo.create(
            second,
            book,
            "Existing discussion",
            &generate_session_token(),
            &[9; 32],
        )
        .await,
        CreateResult::Created(_)
    ));
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let path = format!("/books/{book}/comments");
    let discover = format!("/api/v1/social/mentions/discover?book_id={book}&q=Sec");
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let form = |body: &str| {
        format!(
            "body={body}&command_key={}&csrf_token={csrf}",
            generate_session_token()
        )
    };

    // A leaked URL and an existing comment do not grant an 18+ reader gate.
    pg(&url, &format!("UPDATE books SET access_level='adult' WHERE id={book}")).await;
    assert_eq!(
        call(&app, "GET", &path, "", ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &path, &cookie, ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, &form("blocked"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let denied_mentions = html(call(&app, "GET", &discover, &cookie, ORIGIN, "").await).await;
    assert!(!denied_mentions.contains("Second Reader"));

    pg(&url, &format!("UPDATE users SET adult_enabled=TRUE WHERE id={writer}")).await;
    let readable = call(&app, "GET", &path, &cookie, ORIGIN, "").await;
    assert_eq!(readable.status(), StatusCode::OK);
    let body = html(readable).await;
    assert!(body.contains("S020 gated title"));
    assert!(body.contains("Existing discussion"));
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, &form("adult"))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let mentions = html(call(&app, "GET", &discover, &cookie, ORIGIN, "").await).await;
    assert!(mentions.contains("Second Reader"));
    pg(&url, &format!("UPDATE users SET adult_enabled=FALSE WHERE id={writer}")).await;
    assert_eq!(
        call(&app, "GET", &path, &cookie, ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    let revoked_mentions = html(call(&app, "GET", &discover, &cookie, ORIGIN, "").await).await;
    assert!(!revoked_mentions.contains("Second Reader"));

    // A 21+ grant is independent from the 18+ toggle.
    pg(&url, &format!("UPDATE books SET access_level='pro_adult' WHERE id={book}")).await;
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, &form("denied"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({writer},'pro_adult','moderation',TRUE)
             ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE"
        ),
    )
    .await;
    assert_eq!(
        call(&app, "GET", &path, &cookie, ORIGIN, "").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, &form("pro"))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    pg(
        &url,
        &format!("UPDATE user_level_grants SET is_active=FALSE WHERE user_id={writer}"),
    )
    .await;
    assert_eq!(
        call(&app, "GET", &path, &cookie, ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );

    // Legacy NULL must be fail-closed even if it has retained comments.
    pg(&url, &format!("UPDATE books SET access_level=NULL WHERE id={book}")).await;
    assert_eq!(
        call(&app, "GET", &path, &cookie, ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, &form("unknown"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(&url, &format!("SELECT count(*) FROM discussion_comments WHERE book_id={book}")).await,
        "3",
        "revocations did not delete legitimate prior comments or add forbidden comments"
    );

    pg(&url, &format!("DELETE FROM discussion_comment_mentions WHERE comment_id IN (SELECT id FROM discussion_comments WHERE book_id={book})")).await;
    pg(&url, &format!("DELETE FROM discussion_comments WHERE book_id={book}")).await;
    pg(&url, &format!("DELETE FROM user_level_grants WHERE user_id={writer}")).await;
    pg(&url, &format!("DELETE FROM users WHERE id IN ({writer},{second})")).await;
    pg(&url, &format!("DELETE FROM books WHERE id={book}")).await;
    pool.close().await;
}
