//! F-12 / S046: real PostgreSQL achievements from trusted C completion facts.
//! Only a disposable tor_ci DB; no server, wallet or production migrations.
use sqlx::PgPool;
use tor_recommendations::repositories::{
    AchievementStoreError, PgAchievements,
};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in tor_ci"]
async fn f_12_pg_completion_grants_once_and_forged_event_grants_nothing() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.expect("connect to tor_ci");

    sqlx::query(
        "INSERT INTO users(id,email_normalized,display_name,password_hash)
         OVERRIDING SYSTEM VALUE VALUES
         (99701,'f12-reader-a@example.invalid','Reader A','$argon2id$fixture'),
         (99702,'f12-reader-b@example.invalid','Reader B','$argon2id$fixture')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books(id,source_identity,target_language,title,visibility)
         VALUES(99710,'ci:f12:book','ru','Тестовая книга','public')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES(99720,99710,1,'Тестовая глава','public','free',0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
         VALUES(99730,99720,1,'Проверенная глава')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES(99720,99730)")
        .execute(&pool)
        .await
        .unwrap();

    let event_id: String = sqlx::query_scalar(
        "INSERT INTO reader_chapter_completions
           (user_id,book_id,chapter_id,first_version_id,source_event_id)
         VALUES(99701,99710,99720,99730,gen_random_uuid())
         RETURNING source_event_id::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO outbox_events
           (id,event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         VALUES($1::uuid,'ChapterCompletedV1',1,'chapter',99720,
                'ci:f12:chapter-completed:99701:99720',
                jsonb_build_object(
                  'user_id',99701,'book_id',99710,'chapter_id',99720,
                  'version_id',99730,'authority','C-verified-free-position'),
                ARRAY['achievements-v1'])",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();

    let repo = PgAchievements::new(pool.clone());
    assert!(matches!(
        repo.snapshot(0).await,
        Err(AchievementStoreError::Invalid)
    ));
    let pending = repo.pending_completion_event_ids().await.unwrap();
    assert!(pending.contains(&event_id));
    assert!(repo.accept_completed_chapter(&event_id).await.unwrap());
    assert!(!repo.accept_completed_chapter(&event_id).await.unwrap());
    let view = repo.snapshot(99701).await.unwrap();
    assert_eq!(view.completed_chapters, 1);
    assert_eq!(view.badges.len(), 6);
    assert_eq!(view.badges[0].definition.key, "reader_chapters_1");
    assert!(view.badges[0].unlocked_on.is_some());
    assert!(view.badges[1].unlocked_on.is_none());
    assert!(repo.snapshot(99702).await.unwrap().badges[0].unlocked_on.is_none());
    let actual_grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_achievement_grants WHERE user_id=99701")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(actual_grants, 1);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries
         WHERE event_id=$1::uuid AND consumer='achievements-v1'",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 1);
    let delivered: bool = sqlx::query_scalar(
        "SELECT delivered_at IS NOT NULL FROM outbox_events WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(delivered, "the sole required consumer was acknowledged");

    // Without C's independent canonical completion, an outbox JSON object
    // cannot grant an achievement, even if it claims the trusted event type.
    let forged: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
           (event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         VALUES('ChapterCompletedV1',1,'chapter',99720,
                'ci:f12:forged:99701',jsonb_build_object(
                  'user_id',99701,'book_id',99710,'chapter_id',99720,
                  'version_id',99730,'authority','C-verified-free-position'),
                ARRAY['achievements-v1'])
         RETURNING id::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(matches!(
        repo.accept_completed_chapter(&forged).await,
        Err(AchievementStoreError::Invalid)
    ));
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_achievement_grants WHERE user_id=99701")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, 1, "forged event cannot issue another grant");

    // Cleanup in dependency order so this test can coexist with other PG suites.
    sqlx::query("DELETE FROM event_deliveries WHERE event_id=$1::uuid")
        .bind(&event_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id IN (99701,99702)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=99710")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox_events WHERE id=$1::uuid OR id=$2::uuid")
        .bind(&event_id)
        .bind(&forged)
        .execute(&pool)
        .await
        .unwrap();
}
