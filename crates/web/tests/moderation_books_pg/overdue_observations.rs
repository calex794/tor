//! #661 stage 1: persisted internal overdue observations, without delivery.
use super::*;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn overdue_observations_are_deduplicated_and_cancelled_after_review() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("OverdueInternalObservations2026!").unwrap();
    let token = generate_session_token();
    let owner = accounts
        .register_with_session(
            &format!("mod-overdue-{nonce}@example.invalid"),
            "Owner of test-only pending requests",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();

    let registration: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books
             (source_identity,target_language,title,visibility,visibility_status,
              registration_requires_review,owner_user_id,access_level,created_at)
             VALUES('ci:mod-overdue-reg:{nonce}','ru','Overdue registration',
                    'draft','pending_moderation',true,{owner},'basic',
                    now()-interval '80 hours') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let cover_book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books
             (source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:mod-overdue-cover:{nonce}','ru','Cover candidate',
                    'draft',{owner},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let upload: i64 = pg(
        &url,
        &format!(
            "INSERT INTO cover_upload_objects
             (book_id,uploader_user_id,storage_key,content_sha256,
              content_type,size_bytes,status)
             VALUES({cover_book},{owner},'staging/covers/ops-{nonce}',
                    decode(repeat('ab',32),'hex'),'image/webp',1024,'verified')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let job: i64 = pg(
        &url,
        &format!(
            "INSERT INTO cover_variant_jobs
             (source_object_id,book_id,actor_user_id,recipe_version,expected_slot_revision,status)
             VALUES({upload},{cover_book},{owner},'ci-recipe-overdue',0,'ready')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let cover_request: i64 = pg(
        &url,
        &format!(
            "INSERT INTO book_cover_review_requests
             (book_id,job_id,submitted_by_user_id,expected_slot_revision,submitted_at)
             VALUES({cover_book},{job},{owner},0,now()-interval '30 hours')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let deletion_book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books
             (source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:mod-overdue-del:{nonce}','ru','Pending deletion',
                    'draft',{owner},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "UPDATE books SET lifecycle_status='pending_deletion',lifecycle_revision=2
             WHERE id={deletion_book}"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO book_ordinary_deletion_requests
             (book_id,request_revision,owner_user_id,previous_lifecycle_status,
              previous_visibility,requested_at)
             VALUES ({deletion_book},2,{owner},'in_progress','draft',
                     now()-interval '80 hours')"
        ),
    );

    assert!(accounts
        .scan_moderation_overdue_observations(0)
        .await
        .is_err());
    // Separate scanner invocations (even on separate workers) serialize
    // their writes and must not duplicate threshold observations.
    let (first, second) = tokio::join!(
        accounts.scan_moderation_overdue_observations(500),
        accounts.scan_moderation_overdue_observations(500),
    );
    first.unwrap();
    second.unwrap();
    let subset = format!(
        "(request_kind='registration' AND source_id={registration} AND source_revision=0)
         OR (request_kind='cover' AND source_id={cover_request} AND source_revision=0)
         OR (request_kind='deletion' AND source_id={deletion_book} AND source_revision=2)"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM moderation_overdue_signals WHERE {subset}")
        ),
        "5",
        "80h registration, 30h cover and 80h deletion generate 2+1+2 events"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM moderation_overdue_signals
                 WHERE ({subset}) AND signal_state='open'"
            )
        ),
        "5"
    );
    accounts
        .scan_moderation_overdue_observations(500)
        .await
        .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM moderation_overdue_signals WHERE {subset}")
        ),
        "5",
        "repeating a scan must not produce duplicate events"
    );

    // Only a live site moderator sees actionable observations. The higher
    // 72-hour bucket suppresses a duplicate 24-hour line for the same request.
    let reviewer_token = generate_session_token();
    let reviewer_id = accounts
        .register_with_session(
            &format!("overdue-reviewer-{nonce}@example.invalid"),
            "Overdue site moderator",
            &password,
            &session_token_digest(&reviewer_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({reviewer_id},'moderator',true)"
        ),
    );
    let reviewer_digest = session_token_digest(&reviewer_token);
    assert!(accounts
        .moderation_overdue_inbox(&session_token_digest(&token), None)
        .await
        .is_err());
    assert!(accounts
        .moderation_overdue_inbox(&reviewer_digest, Some(0))
        .await
        .is_err());
    let inbox = accounts
        .moderation_overdue_inbox(&reviewer_digest, None)
        .await
        .unwrap();
    let own: Vec<_> = inbox
        .iter()
        .filter(|item| {
            (item.request_kind == "registration" && item.source_id == registration)
                || (item.request_kind == "cover" && item.source_id == cover_request)
                || (item.request_kind == "deletion" && item.source_id == deletion_book)
        })
        .collect();
    assert_eq!(own.len(), 3, "show one highest-threshold alert per request");
    assert!(own.iter().any(|item| {
        item.request_kind == "registration" && item.threshold_hours == 72
    }));
    assert!(own.iter().any(|item| item.request_kind == "cover" && item.threshold_hours == 24));
    assert!(own.iter().any(|item| item.request_kind == "deletion" && item.threshold_hours == 72));

    // A site moderator who owns the deletion request must not see it here.
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({owner},'moderator',true)"
        ),
    );
    let owner_inbox = accounts
        .moderation_overdue_inbox(&session_token_digest(&token), None)
        .await
        .unwrap();
    assert!(!owner_inbox.iter().any(|item| {
        item.request_kind == "deletion" && item.source_id == deletion_book
    }));
    pg(
        &url,
        &format!("UPDATE account_roles SET is_active=false WHERE user_id={owner}"),
    );
    pg(
        &url,
        &format!("UPDATE account_roles SET is_active=false WHERE user_id={reviewer_id}"),
    );
    assert!(accounts
        .moderation_overdue_inbox(&reviewer_digest, None)
        .await
        .is_err(), "revoked site-role loses access at the repository boundary");
    pg(
        &url,
        &format!("UPDATE account_roles SET is_active=true WHERE user_id={reviewer_id}"),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let mod_cookie = format!("tor-dev-session={reviewer_token}");
    let owner_cookie = format!("tor-dev-session={token}");
    assert_eq!(
        request(&app, "GET", "/moderation/books/alerts", &owner_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "GET", "/moderation/books/alerts?before=0", &mod_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = request(&app, "GET", "/moderation/books/alerts", &mod_cookie, ORIGIN, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let inbox_html = html(response).await;
    assert!(inbox_html.contains(&format!("Регистрация книги #{registration}")));
    assert!(inbox_html.contains(&format!("Обложка #{cover_request}")));
    assert!(inbox_html.contains(&format!("Удаление #{deletion_book}")));
    assert!(inbox_html.contains("/moderation/books/deletions?wait=gte72"));

    // A resolution cannot send stale alerts: reconciliation closes both
    // 24h/72h observations without deleting historical evidence.
    pg(
        &url,
        &format!("UPDATE books SET visibility_status='hidden' WHERE id={registration}"),
    );
    pg(
        &url,
        &format!(
            "UPDATE book_cover_review_requests
             SET state='superseded',superseded_at=clock_timestamp()
             WHERE id={cover_request}"
        ),
    );
    pg(
        &url,
        &format!("UPDATE books SET lifecycle_status='in_progress' WHERE id={deletion_book}"),
    );
    accounts
        .scan_moderation_overdue_observations(500)
        .await
        .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM moderation_overdue_signals
                 WHERE ({subset}) AND signal_state='resolved' AND resolved_at IS NOT NULL"
            )
        ),
        "5",
        "resolved and superseded requests cannot remain due"
    );
    accounts
        .scan_moderation_overdue_observations(500)
        .await
        .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM moderation_overdue_signals WHERE {subset}")
        ),
        "5",
        "closed observation keys remain immutable on subsequent scans"
    );
    let refreshed = request(&app, "GET", "/moderation/books/alerts", &mod_cookie, ORIGIN, "").await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    let page = html(refreshed).await;
    assert!(!page.contains(&format!("Регистрация книги #{registration}")));
    assert!(!page.contains(&format!("Обложка #{cover_request}")));
    assert!(!page.contains(&format!("Удаление #{deletion_book}")));
    pool.close().await;
}
