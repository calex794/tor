//! Site registration moderation has a separate review, not visibility bypass.
use super::*;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn initial_book_registration_requires_site_approval_and_keeps_private_reviewer() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("SiteModeratorApproval2026!").unwrap();
    let ttl = NonZeroU32::new(3600).unwrap();
    let admin_token = generate_session_token();
    let owner_token = generate_session_token();
    let csrf = generate_session_token();
    let admin = accounts
        .register_with_session(
            &format!("mod-reg-admin-{nonce}@example.invalid"),
            "Site Moderator",
            &password,
            &session_token_digest(&admin_token),
            ttl,
        )
        .await
        .unwrap();
    let owner = accounts
        .register_with_session(
            &format!("mod-reg-owner-{nonce}@example.invalid"),
            "Book Owner",
            &password,
            &session_token_digest(&owner_token),
            ttl,
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!(
        "INSERT INTO account_roles(user_id,role,is_active) VALUES({admin},'administrator',true)"
    ),
    );
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books
             (source_identity,target_language,title,visibility,visibility_status,
              registration_requires_review,owner_user_id,access_level)
             VALUES('ci:mod-reg:{nonce}','ru','<script>Await review</script>',
                    'draft','pending_moderation',true,{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let queue_path = "/moderation/books/registrations";
    let decision_path = format!("/moderation/books/{book}/registration");
    let body = format!("csrf_token={csrf}&expected_revision=1&action=approve");

    assert_eq!(
        request(&app, "GET", queue_path, "", ORIGIN, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", queue_path, &owner_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", &decision_path, &owner_cookie, ORIGIN, &body)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &decision_path,
            &admin_cookie,
            "https://evil.invalid",
            &body
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let queue = request(&app, "GET", queue_path, &admin_cookie, ORIGIN, "").await;
    assert_eq!(queue.status(), StatusCode::OK);
    assert_eq!(queue.headers()[header::CACHE_CONTROL], "no-store");
    let content = html(queue).await;
    assert!(content.contains("&lt;script&gt;Await review&lt;/script&gt;"));
    assert!(!content.contains("<script>Await review</script>"));
    assert!(content.contains("Регистрация ("));
    assert!(content.contains(&decision_path));

    // Search is scoped to all pending Book requests before LIMIT 40, not a
    // client-side filter over the first 40 returned cards.
    let searched = request(
        &app,
        "GET",
        &format!("{queue_path}?q={book}"),
        &admin_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(searched.status(), StatusCode::OK);
    assert!(html(searched).await.contains(&decision_path));
    let missing = request(
        &app,
        "GET",
        &format!("{queue_path}?q=missing-review-title-{nonce}"),
        &admin_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(missing.status(), StatusCode::OK);
    assert!(!html(missing).await.contains(&decision_path));

    // Age-level restriction runs in PostgreSQL, before the 40-item limit.
    // The test Book is basic; 18+/21+ and legacy NULL must not match.
    for (tier, should_include) in [
        ("basic", true),
        ("adult", false),
        ("pro_adult", false),
        ("legacy", false),
    ] {
        let result = request(
            &app,
            "GET",
            &format!("{queue_path}?level={tier}"),
            &admin_cookie,
            ORIGIN,
            "",
        )
        .await;
        assert_eq!(result.status(), StatusCode::OK);
        assert_eq!(
            html(result).await.contains(&decision_path),
            should_include,
            "unexpected registration queue tier {tier}"
        );
    }
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("{queue_path}?level=untrusted"),
            &admin_cookie,
            ORIGIN,
            "",
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        request(
            &app, "POST", &format!("/moderation/books/{book}/visibility"),
            &admin_cookie, ORIGIN,
            &format!("csrf_token={csrf}&expected_status=pending_moderation&expected_revision=1&new_status=active")
        ).await.status(),
        StatusCode::CONFLICT,
        "generic visibility editor cannot bypass initial registration approval"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility||':'||visibility_status FROM books WHERE id={book}")
        ),
        "draft:pending_moderation"
    );
    assert_eq!(
        request(&app, "POST", &decision_path, &admin_cookie, ORIGIN, &body)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(&app, "POST", &decision_path, &admin_cookie, ORIGIN, &body)
            .await
            .status(),
        StatusCode::CONFLICT,
        "double decision must not rewrite reviewer or audit"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility||':'||visibility_status FROM books WHERE id={book}")
        ),
        "draft:active",
        "registration review must never publish the author's draft"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT reviewer_user_id FROM book_registration_decisions
             WHERE book_id={book} AND outcome='approved'"
            )
        ),
        admin.to_string()
    );
    let private_path = format!("/moderation/books/{book}/registration-history");
    assert_eq!(
        request(&app, "GET", &private_path, &owner_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let private = html(request(&app, "GET", &private_path, &admin_cookie, ORIGIN, "").await).await;
    assert!(private.contains("Site Moderator"));
    assert!(private.contains(&admin.to_string()));
    assert_eq!(
        request(&app, "GET", &format!("/books/{book}"), "", ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let refused: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books
             (source_identity,target_language,title,visibility,visibility_status,
              registration_requires_review,owner_user_id,access_level)
             VALUES('ci:mod-reg-rejected:{nonce}','ru','Refused','draft',
                    'pending_moderation',true,{owner},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let rejected_path = format!("/moderation/books/{refused}/registration");
    assert_eq!(
        request(
            &app,
            "POST",
            &rejected_path,
            &admin_cookie,
            ORIGIN,
            &format!("csrf_token={csrf}&expected_revision=1&action=reject")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &rejected_path,
            &admin_cookie,
            ORIGIN,
            &format!("csrf_token={csrf}&expected_revision=1&action=reject&reason=Incorrect+book")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility_status FROM books WHERE id={refused}")
        ),
        "hidden"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
            "SELECT outcome||':'||reason FROM book_registration_decisions WHERE book_id={refused}"
        )
        ),
        "rejected:Incorrect book"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT COUNT(*) FROM book_registration_decisions WHERE book_id={book}")
        ),
        "1"
    );
    // The owner learns *why* registration was rejected, not who reviewed it.
    let refused_status_path = format!("/me/books/{refused}/registration-status");
    assert_eq!(
        request(&app, "GET", &refused_status_path, "", ORIGIN, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", &refused_status_path, &admin_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "site-role alone does not grant owner-facing application data"
    );
    let owner_status = request(&app, "GET", &refused_status_path, &owner_cookie, ORIGIN, "").await;
    assert_eq!(owner_status.status(), StatusCode::OK);
    assert_eq!(owner_status.headers()[header::CACHE_CONTROL], "no-store");
    let owner_reason = html(owner_status).await;
    assert!(owner_reason.contains("Причина отказа"));
    assert!(owner_reason.contains("Incorrect book"));
    assert!(!owner_reason.contains("Site Moderator"));
    assert!(!owner_reason.contains(&format!("(#{admin})")));
    let accepted_status = html(
        request(
            &app,
            "GET",
            &format!("/me/books/{book}/registration-status"),
            &owner_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(accepted_status.contains("Регистрация одобрена"));
    assert!(!accepted_status.contains("Site Moderator"));
    pool.close().await;
}
