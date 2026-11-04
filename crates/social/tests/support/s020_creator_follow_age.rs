//! P0 #486: E-05 creator follows may not expose Book titles after age revocation.
use super::*;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending with /tor_ci"]
async fn e_05_s020_creator_follow_and_source_titles_recheck_live_age_grants() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable test DB");
    assert!(url.ends_with("/tor_ci"), "production DB forbidden");
    let pool = PgPool::connect(&url).await.expect("connect");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut actors = Vec::new();
    for suffix in ["creator", "reader"] {
        let actor: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,$2,'$argon2id$fixture') RETURNING id",
        )
        .bind(format!("e05-age-{stamp}-{suffix}@example.invalid"))
        .bind(format!("E05 Age {suffix}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        actors.push(actor);
    }
    let creator = actors[0];
    let reader = actors[1];
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','S020 E05 restricted source','public',$2,'adult') RETURNING id",
    )
    .bind(format!("ci:e05:age:{stamp}"))
    .bind(creator)
    .fetch_one(&pool)
    .await
    .unwrap();
    let follows = PgCreatorFollows::new(pool.clone());

    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound),
        "a known adult Book ID does not authorize a creator follow"
    );
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await.unwrap(),
        creator
    );
    let title = follows.list_active(reader, None).await.unwrap();
    assert_eq!(title.len(), 1);
    assert_eq!(
        title[0].source_book.as_ref().map(|pair| pair.0),
        Some(book)
    );

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    let hidden = follows.list_active(reader, None).await.unwrap();
    assert_eq!(hidden.len(), 1, "retain the creator follow period");
    assert!(hidden[0].source_book.is_none(), "hide revoked adult title");
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound)
    );
    assert_eq!(follows.unfollow(reader, creator).await, Ok(()));
    assert!(follows.list_active(reader, None).await.unwrap().is_empty());

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound),
        "adult opt-in alone never grants the independent 21+ tier"
    );
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(reader)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await.unwrap(),
        creator
    );
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_some());
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_none());
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound)
    );

    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_none());
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound)
    );

    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)",
    )
    .bind(reader)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await.unwrap(),
        creator
    );
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_some());
    sqlx::query("UPDATE account_roles SET is_active=FALSE WHERE user_id=$1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_none());

    sqlx::query(
        "UPDATE books SET access_level='basic',lifecycle_status='pending_deletion' WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_none());
    sqlx::query("UPDATE books SET lifecycle_status='in_progress' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_some());

    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::Invalid)
    );
    assert!(follows.list_active(reader, None).await.unwrap().is_empty());
    assert_eq!(follows.unfollow(reader, creator).await, Ok(()));
    pool.close().await;
}
