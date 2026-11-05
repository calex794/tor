//! S020/#486: settlement possession does not override live 18+/21+ access.
use sqlx::PgPool;
use tor_reading::repositories::PgReadingSync;

#[tokio::test]
#[ignore = "requires disposable isolated TEST_DATABASE_URL ending in tor_ci"]
async fn s020_pinned_paid_v1_requires_live_age_and_keeps_owned_version_after_hidden() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(db, "tor_ci");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    for suffix in ["buyer", "other"] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'S020 paid tier','$argon2id$test') RETURNING id",
        )
        .bind(format!("s020-paid-age-{nonce}-{suffix}@example.test"))
        .fetch_one(&pool)
        .await
        .unwrap();
        users.push(id);
    }
    let buyer = users[0];
    let other = users[1];
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','S020 Owned V1','public','basic') RETURNING id",
    )
    .bind(format!("ci:s020:paid:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Paid pinned chapter','public','paid',1200) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Paid V1 private marker') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();
    let purchase: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
           (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,$4,1200,'RUB') RETURNING id",
    )
    .bind(buyer)
    .bind(chapter)
    .bind(v1)
    .bind(format!("s020-paid-v1-{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let sync = PgReadingSync::new(pool.clone());
    assert!(sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    assert!(!sync
        .purchased_allowed(other, purchase, chapter, v1)
        .await
        .unwrap());
    assert_eq!(
        sync.purchased_body_for_validation(buyer, purchase, chapter, v1)
            .await
            .unwrap()
            .as_deref(),
        Some("Paid V1 private marker")
    );

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    assert!(sync
        .purchased_body_for_validation(buyer, purchase, chapter, v1)
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    assert!(sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());

    // NEwBOO: direct pinned purchase remains usable after storefront withdrawal
    // and throughout pending deletion. Only hard blocking/refund revokes.
    sqlx::query(
        "UPDATE books SET access_level='basic',visibility='draft',
            visibility_status='hidden',lifecycle_status='pending_deletion'
         WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert!(sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());

    // BR-060: a later paid V2 must not rewrite the immutable V1 purchase.
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'New paid V2 without a settlement') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    assert!(!sync
        .purchased_allowed(buyer, purchase, chapter, v2)
        .await
        .unwrap());

    // Completed *ordinary* deletion must deindex the Book without changing
    // the exact buyer-owned V1, but must not grant any unsold later edition.
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         ordinary_deleted_at=now() WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert!(sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    assert!(!sync
        .purchased_allowed(other, purchase, chapter, v1)
        .await
        .unwrap());
    assert_eq!(
        sync.purchased_body_for_validation(buyer, purchase, chapter, v1)
            .await
            .unwrap()
            .as_deref(),
        Some("Paid V1 private marker")
    );

    // Live age revocation still wins over an otherwise retained purchase.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());

    sqlx::query("UPDATE books SET visibility_status='blocked' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());

    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,$2,'refund')",
    )
    .bind(purchase)
    .bind(format!("s020-refund-{nonce}"))
    .execute(&pool)
    .await
    .unwrap();
    assert!(!sync
        .purchased_allowed(buyer, purchase, chapter, v1)
        .await
        .unwrap());
    let retained: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM library_purchased_chapters
         WHERE user_id=$1 AND chapter_id=$2 AND active_version_id=$3",
    )
    .bind(buyer)
    .bind(chapter)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        retained, 1,
        "refund must not rewrite immutable purchase history"
    );
}
