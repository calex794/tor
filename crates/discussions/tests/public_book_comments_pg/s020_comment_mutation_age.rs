//! S020/#486: Book comments and mention discovery cannot bypass the live
//! Book tier through direct E-02 repository requests.
use super::*;
use sqlx::PgPool;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in tor_ci"]
async fn e_02_age_tier_mutations_recheck_reader_rights_without_leaking_mentions() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'E02 age viewer','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("e02-age-viewer-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let target: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Reader Candidate','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("e02-age-target-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,visibility_status,access_level)
         VALUES($1,'ru','Restricted public Book','public','active','basic') RETURNING id",
    )
    .bind(format!("ci:e02:age-mutations:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgPublicBookComments::new(pool.clone());
    assert!(matches!(
        repo.create(target, book, "Reader initial remark", &"a".repeat(43), &[1; 32])
            .await,
        CreateResult::Created(_)
    ));
    assert_eq!(
        repo.mention_candidates(actor, book, "Read")
            .await
            .unwrap()
            .iter()
            .map(|candidate| candidate.user_id)
            .collect::<Vec<_>>(),
        vec![target]
    );

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.create(actor, book, "Denied adult", &"b".repeat(43), &[2; 32])
            .await,
        CreateResult::NotFound
    );
    assert!(repo
        .mention_candidates(actor, book, "Read")
        .await
        .unwrap()
        .is_empty());

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repo.create(actor, book, "Adult allowed", &"c".repeat(43), &[3; 32])
            .await,
        CreateResult::Created(_)
    ));
    assert_eq!(repo.mention_candidates(actor, book, "Read").await.unwrap().len(), 1);

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.create(actor, book, "Denied without grant", &"d".repeat(43), &[4; 32])
            .await,
        CreateResult::NotFound
    );
    assert!(repo.mention_candidates(actor, book, "Read").await.unwrap().is_empty());

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        repo.create(actor, book, "Pro allowed", &"e".repeat(43), &[5; 32])
            .await,
        CreateResult::Created(_)
    ));
    assert_eq!(repo.mention_candidates(actor, book, "Read").await.unwrap().len(), 1);

    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.create(actor, book, "Revoked grant", &"f".repeat(43), &[6; 32])
            .await,
        CreateResult::NotFound
    );
    assert!(repo.mention_candidates(actor, book, "Read").await.unwrap().is_empty());

    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.create(actor, book, "Legacy must fail", &"g".repeat(43), &[7; 32])
            .await,
        CreateResult::NotFound
    );

    // A site administrator has the independent NEwBOO age bypass;
    // a site moderator never inherits it.
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'moderator',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.create(actor, book, "Moderator denied", &"h".repeat(43), &[8; 32])
            .await,
        CreateResult::NotFound
    );
    sqlx::query("UPDATE account_roles SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        repo.create(actor, book, "Admin age bypass", &"i".repeat(43), &[9; 32])
            .await,
        CreateResult::Created(_)
    ));
    assert_eq!(repo.mention_candidates(actor, book, "Read").await.unwrap().len(), 1);

    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.create(actor, book, "Suspended admin", &"j".repeat(43), &[10; 32])
            .await,
        CreateResult::NotFound
    );
    assert!(repo.mention_candidates(actor, book, "Read").await.unwrap().is_empty());

    sqlx::query("UPDATE users SET account_state='active' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET access_level='basic',lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.create(actor, book, "No new public comment", &"k".repeat(43), &[11; 32])
            .await,
        CreateResult::NotFound
    );
    assert!(repo.mention_candidates(actor, book, "Read").await.unwrap().is_empty());

    // All denied requests leave the source discussion untouched.
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discussion_comments WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 4);
}
