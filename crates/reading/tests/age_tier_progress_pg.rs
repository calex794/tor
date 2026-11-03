//! S020/#486: C-04 progress and C-09 free sync cannot outlive an age grant.
use sqlx::PgPool;
use tor_reading::{
    repositories::{PgReadingProgress, PgReadingSync},
    services::{
        progress::{ProgressError, ReadingPlace, SavedPlaceState},
        sync::SyncError,
    },
};

#[tokio::test]
#[ignore = "requires disposable isolated PostgreSQL tor_ci"]
async fn s020_progress_and_free_sync_recheck_live_age_opt_in_and_grants() {
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

    let mut actors = Vec::<i64>::new();
    for suffix in ["progress", "sync"] {
        let actor: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES ($1,'S020 age progress','$argon2id$test') RETURNING id",
        )
        .bind(format!("s020-progress-{nonce}-{suffix}@example.test"))
        .fetch_one(&pool)
        .await
        .unwrap();
        actors.push(actor);
    }
    let progress_actor = actors[0];
    let sync_actor = actors[1];
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES ($1,'ru','S020 Age Cursor','public','basic') RETURNING id",
    )
    .bind(format!("ci:s020:progress:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES ($1,1,'S020 Free Cursor','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES ($1,1,'Первый абзац\n\nВторой абзац') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES ($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();

    let progress = PgReadingProgress::new(pool.clone());
    let sync = PgReadingSync::new(pool.clone());
    let first = ReadingPlace {
        block_index: 1,
        char_offset: 1,
    };
    let later = ReadingPlace {
        block_index: 1,
        char_offset: 2,
    };
    progress
        .save_position(progress_actor, chapter, version, first)
        .await
        .unwrap();
    assert_eq!(
        progress.saved_state(progress_actor, chapter, version).await.unwrap(),
        SavedPlaceState::Current(first)
    );
    assert!(progress
        .continue_reading(progress_actor)
        .await
        .unwrap()
        .iter()
        .any(|item| item.book_id == book));
    assert_eq!(
        sync.save_free(sync_actor, chapter, version, first, 0)
            .await
            .unwrap()
            .revision,
        1
    );
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_some());

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress.continue_reading(progress_actor).await.unwrap().is_empty());
    assert_eq!(
        progress.saved_state(progress_actor, chapter, version).await.unwrap(),
        SavedPlaceState::NotSaved
    );
    assert_eq!(
        progress.save_position(progress_actor, chapter, version, later).await,
        Err(ProgressError::NotFound)
    );
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_none());
    assert_eq!(
        sync.save_free(sync_actor, chapter, version, later, 1).await,
        Err(SyncError::Conflict)
    );
    let preserved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reader_book_positions WHERE book_id=$1 AND user_id=ANY($2::bigint[])",
    )
    .bind(book)
    .bind(&actors)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved, 2, "revocation hides, never deletes saved progress");

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(progress_actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress
        .continue_reading(progress_actor)
        .await
        .unwrap()
        .iter()
        .any(|item| item.book_id == book));
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_none());
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(sync_actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_some());

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress.continue_reading(progress_actor).await.unwrap().is_empty());
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_none());
    for actor in actors.iter().copied() {
        sqlx::query(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES($1,'pro_adult','moderation',TRUE)",
        )
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    }
    assert!(progress
        .continue_reading(progress_actor)
        .await
        .unwrap()
        .iter()
        .any(|item| item.book_id == book));
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_some());
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(sync_actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_none());

    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress.continue_reading(progress_actor).await.unwrap().is_empty());
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_none());
    // NEwBOO parity: pending deletion unlists the Book, but direct reading and
    // already-owned cursors remain valid until hard deletion or site blocking.
    sqlx::query("UPDATE books SET access_level='basic',lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress
        .continue_reading(progress_actor)
        .await
        .unwrap()
        .iter()
        .any(|item| item.book_id == book));
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_some());
    sqlx::query("UPDATE books SET visibility_status='blocked' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress.continue_reading(progress_actor).await.unwrap().is_empty());
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_none());
    sqlx::query("UPDATE books SET visibility_status='active' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET lifecycle_status='in_progress' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress
        .continue_reading(progress_actor)
        .await
        .unwrap()
        .iter()
        .any(|item| item.book_id == book));
    assert!(sync.free_state(sync_actor, chapter).await.unwrap().is_some());
}
