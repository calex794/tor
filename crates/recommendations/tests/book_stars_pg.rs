//! F-05: real PostgreSQL 18 work and translation scores. Disposable tor_ci only.
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_recommendations::repositories::{PgBookStars, StarKind, StarRatingError, StarTotal};

async fn fixture(label: &str) -> (PgBookStars, PgPool, i64, i64, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI URL");
    assert!(
        url.ends_with("/tor_ci"),
        "never run ratings against production"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let key = format!("f05:book-stars:{label}:{nonce}");
    let mut readers = Vec::new();
    for index in 0..2 {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'F05 reader','$argon2id$test') RETURNING id",
        )
        .bind(format!("{key}:{index}@tor-ci.example"))
        .fetch_one(&pool)
        .await
        .unwrap();
        readers.push(id);
    }
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','Проверяемая книга','public','basic') RETURNING id",
    )
    .bind(&key)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Глава 1','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Текст') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    for user in &readers {
        sqlx::query(
            "INSERT INTO reader_chapter_completions
               (user_id,book_id,chapter_id,first_version_id,source_event_id)
             VALUES($1,$2,$3,$4,gen_random_uuid())",
        )
        .bind(user)
        .bind(book)
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    }
    (
        PgBookStars::new(pool.clone()),
        pool,
        readers[0],
        readers[1],
        book,
        chapter,
    )
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn f_05_pg_work_and_translation_are_independent_updatable_and_private() {
    let (store, pool, first, second, book, _) = fixture("independent").await;
    assert_eq!(
        store.set_rating(first, book, StarKind::Work, 0).await,
        Err(StarRatingError::Invalid)
    );
    assert_eq!(
        store.set_rating(first, book, StarKind::Work, 6).await,
        Err(StarRatingError::Invalid)
    );
    store
        .set_rating(first, book, StarKind::Work, 5)
        .await
        .unwrap();
    store
        .set_rating(first, book, StarKind::Translation, 2)
        .await
        .unwrap();
    store
        .set_rating(first, book, StarKind::Work, 5)
        .await
        .unwrap();
    let before = store.visible_summary(book).await.unwrap().unwrap();
    assert_eq!(before.work, StarTotal { sum: 5, votes: 1 });
    assert_eq!(before.translation, StarTotal { sum: 2, votes: 1 });
    store
        .set_rating(first, book, StarKind::Work, 3)
        .await
        .unwrap();
    store
        .set_rating(second, book, StarKind::Work, 1)
        .await
        .unwrap();
    let after = PgBookStars::new(pool.clone())
        .visible_summary(book)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.work, StarTotal { sum: 4, votes: 2 });
    assert_eq!(after.work.display_average().as_deref(), Some("2.00"));
    assert_eq!(after.translation, StarTotal { sum: 2, votes: 1 });
    let mine = store.own_ratings(first, book).await.unwrap();
    assert_eq!((mine.work, mine.translation), (Some(3), Some(2)));
    let theirs = store.own_ratings(second, book).await.unwrap();
    assert_eq!((theirs.work, theirs.translation), (Some(1), None));
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.visible_summary(book).await.unwrap().is_none());
    assert!(!store.eligible(first, book).await.unwrap());
    assert_eq!(
        store.set_rating(first, book, StarKind::Work, 4).await,
        Err(StarRatingError::NotEligible)
    );
    assert_eq!(store.own_ratings(first, book).await.unwrap().work, None);
    sqlx::query("UPDATE books SET visibility_status='active' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .visible_summary(book)
            .await
            .unwrap()
            .unwrap()
            .work
            .votes,
        2
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn f_05_pg_two_devices_and_user_erasure_never_duplicate_averages() {
    let (store, pool, first, _, book, _) = fixture("parallel").await;
    let (one, two) = tokio::join!(
        store.set_rating(first, book, StarKind::Work, 4),
        store.set_rating(first, book, StarKind::Work, 1),
    );
    assert_eq!(one, Ok(()));
    assert_eq!(two, Ok(()));
    let summary = store.visible_summary(book).await.unwrap().unwrap();
    assert_eq!(summary.work.votes, 1);
    assert!([1_i64, 4].contains(&summary.work.sum));
    assert_eq!(summary.translation.votes, 0);
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    let emptied = store.visible_summary(book).await.unwrap().unwrap();
    assert_eq!(emptied.work, StarTotal { sum: 0, votes: 0 });
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn f_05_pg_hidden_or_unverified_books_cannot_collect_or_expose_votes() {
    let (store, pool, first, _, book, chapter) = fixture("guards").await;
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Unread','$argon2id$test') RETURNING id",
    )
    .bind(format!("f05-unread-{book}@tor-ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        store.set_rating(other, book, StarKind::Work, 5).await,
        Err(StarRatingError::NotEligible),
        "opening a page or S3 grant cannot invent C completion"
    );
    store
        .set_rating(first, book, StarKind::Translation, 4)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(store.visible_summary(book).await.unwrap(), None);
    assert_eq!(
        store
            .set_rating(first, book, StarKind::Translation, 2)
            .await,
        Err(StarRatingError::NotEligible)
    );
    assert_eq!(
        store.own_ratings(first, book).await.unwrap().translation,
        None
    );
    // Completion events are durable history; moderation still prevents new votes.
    let completed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reader_chapter_completions
         WHERE user_id=$1 AND book_id=$2 AND chapter_id=$3",
    )
    .bind(first)
    .bind(book)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed, 1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn f_05_s020_live_age_rating_acl_does_not_revoke_saved_stars() {
    let (store, pool, actor, _, book, _) = fixture("s020-tiers").await;
    store
        .set_rating(actor, book, StarKind::Work, 5)
        .await
        .unwrap();
    assert!(store.visible_summary(book).await.unwrap().is_some());
    assert!(store
        .visible_summary_for_actor(actor, book)
        .await
        .unwrap()
        .is_some());
    assert!(store.eligible(actor, book).await.unwrap());

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(store.visible_summary(book).await.unwrap(), None);
    assert_eq!(
        store.visible_summary_for_actor(actor, book).await.unwrap(),
        None
    );
    assert!(!store.eligible(actor, book).await.unwrap());
    assert_eq!(
        store.own_ratings(actor, book).await.unwrap(),
        Default::default()
    );
    assert_eq!(
        store.set_rating(actor, book, StarKind::Work, 2).await,
        Err(StarRatingError::NotEligible)
    );

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store
        .visible_summary_for_actor(actor, book)
        .await
        .unwrap()
        .is_some());
    assert_eq!(store.visible_summary(book).await.unwrap(), None);
    assert!(store.eligible(actor, book).await.unwrap());
    store
        .set_rating(actor, book, StarKind::Work, 4)
        .await
        .unwrap();

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.visible_summary_for_actor(actor, book).await.unwrap(),
        None
    );
    assert_eq!(
        store.set_rating(actor, book, StarKind::Work, 2).await,
        Err(StarRatingError::NotEligible)
    );
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store
        .visible_summary_for_actor(actor, book)
        .await
        .unwrap()
        .is_some());
    assert!(store.eligible(actor, book).await.unwrap());
    store
        .set_rating(actor, book, StarKind::Work, 3)
        .await
        .unwrap();

    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.visible_summary_for_actor(actor, book).await.unwrap(),
        None
    );
    assert!(!store.eligible(actor, book).await.unwrap());
    assert_eq!(
        store.set_rating(actor, book, StarKind::Work, 1).await,
        Err(StarRatingError::NotEligible)
    );

    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.visible_summary_for_actor(actor, book).await.unwrap(),
        None
    );

    sqlx::query("INSERT INTO account_roles(user_id,role,is_active) VALUES($1,'moderator',TRUE)")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.visible_summary_for_actor(actor, book).await.unwrap(),
        None
    );
    assert_eq!(
        store
            .set_rating(actor, book, StarKind::Translation, 5)
            .await,
        Err(StarRatingError::NotEligible)
    );
    sqlx::query("UPDATE account_roles SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active) VALUES($1,'administrator',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(store
        .visible_summary_for_actor(actor, book)
        .await
        .unwrap()
        .is_some());
    assert!(store.eligible(actor, book).await.unwrap());

    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.visible_summary_for_actor(actor, book).await.unwrap(),
        None
    );
    assert!(!store.eligible(actor, book).await.unwrap());
    assert_eq!(
        store.set_rating(actor, book, StarKind::Work, 1).await,
        Err(StarRatingError::NotEligible)
    );
    let saved: (i64, i16) = sqlx::query_as(
        "SELECT count(*)::bigint,max(rating) FROM user_book_star_ratings
         WHERE book_id=$1 AND user_id=$2 AND rating_kind='work'",
    )
    .bind(book)
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved, (1, 3), "revocation never erases historical votes");
    pool.close().await;
}
