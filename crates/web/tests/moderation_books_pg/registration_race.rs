//! MOD-REG-01: two independent site reviewers race on one pending registration.
//! Exactly one decision may change the Book; the other must receive 409.
use super::*;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn independent_reviewers_cannot_both_decide_one_registration() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("ModRegistrationRace2026!").unwrap();
    let ttl = NonZeroU32::new(3600).unwrap();
    let token_a = generate_session_token();
    let token_b = generate_session_token();
    let owner_token = generate_session_token();
    let csrf_a = generate_session_token();
    let csrf_b = generate_session_token();
    let moderator_a = accounts
        .register_with_session(
            &format!("mod-reg-race-a-{nonce}@example.invalid"),
            "Independent reviewer A",
            &password,
            &session_token_digest(&token_a),
            ttl,
        )
        .await
        .unwrap();
    let moderator_b = accounts
        .register_with_session(
            &format!("mod-reg-race-b-{nonce}@example.invalid"),
            "Independent reviewer B",
            &password,
            &session_token_digest(&token_b),
            ttl,
        )
        .await
        .unwrap();
    let owner = accounts
        .register_with_session(
            &format!("mod-reg-race-owner-{nonce}@example.invalid"),
            "Book registration owner",
            &password,
            &session_token_digest(&owner_token),
            ttl,
        )
        .await
        .unwrap();
    assert_ne!(moderator_a, moderator_b);
    assert_ne!(moderator_a, owner);
    assert_ne!(moderator_b, owner);
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES ({moderator_a},'moderator',true),
                    ({moderator_b},'administrator',true)"
        ),
    );
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books
             (source_identity,target_language,title,visibility,visibility_status,
              registration_requires_review,owner_user_id,access_level)
             VALUES ('ci:mod-reg-race:{nonce}','ru','Review race','draft',
                     'pending_moderation',true,{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let url_path = format!("/moderation/books/{book}/registration");
    let cookie_a = format!("tor-dev-session={token_a}; tor-dev-csrf={csrf_a}");
    let cookie_b = format!("tor-dev-session={token_b}; tor-dev-csrf={csrf_b}");
    let approve = format!("csrf_token={csrf_a}&expected_revision=1&action=approve&confirmed=yes");
    let reject = format!(
        "csrf_token={csrf_b}&expected_revision=1&action=reject&reason=Not+ready&confirmed=yes"
    );
    let (a, b) = tokio::join!(
        request(&app, "POST", &url_path, &cookie_a, ORIGIN, &approve),
        request(&app, "POST", &url_path, &cookie_b, ORIGIN, &reject),
    );
    let status_a = a.status();
    let status_b = b.status();
    assert!(
        (status_a == StatusCode::SEE_OTHER && status_b == StatusCode::CONFLICT)
            || (status_b == StatusCode::SEE_OTHER && status_a == StatusCode::CONFLICT),
        "two site moderators must produce one 303 and one 409: {status_a}/{status_b}"
    );
    let loser = if status_a == StatusCode::CONFLICT { a } else { b };
    assert_eq!(loser.headers()[header::CACHE_CONTROL], "no-store");
    let error_html = html(loser).await;
    assert!(!error_html.contains("Independent reviewer A"));
    assert!(!error_html.contains("Independent reviewer B"));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM book_registration_decisions WHERE book_id={book}")
        ),
        "1",
        "two independent concurrent reviews must append one immutable decision"
    );
    let winner_id = if status_a == StatusCode::SEE_OTHER {
        moderator_a
    } else {
        moderator_b
    };
    let expected = if status_a == StatusCode::SEE_OTHER {
        "approved:active"
    } else {
        "rejected:hidden"
    };
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT outcome||':'||visibility_status
                 FROM book_registration_decisions d JOIN books b ON b.id=d.book_id
                 WHERE d.book_id={book}"
            )
        ),
        expected
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT reviewer_user_id FROM book_registration_decisions WHERE book_id={book}"
            )
        ),
        winner_id.to_string(),
        "the sole decision must be attributed to the actual winning reviewer"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &url_path,
            &cookie_a,
            ORIGIN,
            &approve
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM book_registration_decisions WHERE book_id={book}")
        ),
        "1",
        "a repeated decision must not create a second audit event"
    );
}
