//! C -> F-05 on disposable PostgreSQL: no fake completion by GET or bookmark.
//! The worker and user actions can race; outbox receipt and ranking must commit once.
use sqlx::PgPool;
use tor_reading::{repositories::PgReadingSync, services::progress::ReadingPlace};
use tor_recommendations::{
    repositories::{PgRanking, RankingCommand, RankingStoreError},
    services::ranking::{AnswerStatus, Outcome},
};

async fn completion(sync: &PgReadingSync, pool: &PgPool, chapter_id: i64) -> String {
    let place = ReadingPlace {
        block_index: 2,
        char_offset: 7, // "Конец😀" uses 7 UTF-16 code units.
    };
    sync.save_free(99501, chapter_id, chapter_id + 10, place, 0)
        .await
        .unwrap();
    sqlx::query_scalar(
        "SELECT id::text FROM outbox_events
         WHERE aggregate_kind='chapter' AND aggregate_id=$1
           AND event_type='ChapterCompletedV1'",
    )
    .bind(chapter_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn f_05_c_verified_completion_outbox_to_ranked_books_is_once_and_recoverable() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never test production");
    let pool = PgPool::connect(&url).await.unwrap();

    // Crash-safe re-run of this test: remove its earlier receipts before events.
    sqlx::query(
        "DELETE FROM event_deliveries
         WHERE event_id IN (
            SELECT id FROM outbox_events
            WHERE event_type='ChapterCompletedV1'
              AND aggregate_id BETWEEN 99520 AND 99522
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM outbox_events WHERE event_type='ChapterCompletedV1'
         AND aggregate_id BETWEEN 99520 AND 99522",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM users WHERE id IN (99501,99502)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id IN (99510,99511,99512)")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO users(id,email_normalized,display_name,password_hash)
         OVERRIDING SYSTEM VALUE VALUES
         (99501,'f05-c-reader@example.invalid','F05 C Reader','$argon2id$fixture'),
         (99502,'f05-c-other@example.invalid','F05 Other','$argon2id$fixture')",
    )
    .execute(&pool)
    .await
    .unwrap();
    for i in 0..3_i64 {
        sqlx::query(
            "INSERT INTO books(id,source_identity,target_language,title,visibility)
             VALUES($1,$2,'ru',$3,'public')",
        )
        .bind(99510 + i)
        .bind(format!("ci:f05:completion:{i}"))
        .bind(format!("Прочитанная {i}"))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES($1,$2,1,'Глава','public','free',0)",
        )
        .bind(99520 + i)
        .bind(99510 + i)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
             VALUES($1,$2,1,$3)",
        )
        .bind(99530 + i)
        .bind(99520 + i)
        .bind("Начало\n\nКонец😀")
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)",
        )
        .bind(99520 + i)
        .bind(99530 + i)
        .execute(&pool)
        .await
        .unwrap();
    }

    let rank = PgRanking::new(pool.clone());
    let sync = PgReadingSync::new(pool.clone());
    assert!(rank.pending_completion_event_ids().await.unwrap().is_empty());
    assert_eq!(rank.snapshot(99501).await.unwrap().revision, 0);
    assert_eq!(
        sync.save_free(
            99501, 99520, 99530,
            ReadingPlace { block_index: 1, char_offset: 3 }, 0,
        )
        .await
        .unwrap()
        .revision,
        1
    );
    assert!(rank.pending_completion_event_ids().await.unwrap().is_empty());

    // A validated final progress save produces one C fact and one outbox row.
    let completed = ReadingPlace {
        block_index: 2,
        char_offset: 7,
    };
    assert_eq!(
        sync.save_free(99501, 99520, 99530, completed, 1)
            .await
            .unwrap()
            .revision,
        2
    );
    let first: String = sqlx::query_scalar(
        "SELECT id::text FROM outbox_events
         WHERE aggregate_id=99520 AND event_type='ChapterCompletedV1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(rank.pending_completion_event_ids().await.unwrap().contains(&first));
    assert!(rank.accept_completed_chapter(&first).await.unwrap());
    assert!(!rank.accept_completed_chapter(&first).await.unwrap());
    let snap = PgRanking::new(pool.clone()).snapshot(99501).await.unwrap();
    assert_eq!(snap.ranks.len(), 1);
    assert_eq!(snap.ranks[0].book_id, 99510);
    assert_eq!(snap.revision, 1);
    assert!(rank.snapshot(99502).await.unwrap().ranks.is_empty());

    // Second book starts the real private comparison.
    let second = completion(&sync, &pool, 99521).await;
    assert!(rank.accept_completed_chapter(&second).await.unwrap());
    let active = rank.snapshot(99501).await.unwrap();
    assert_eq!(active.placement.as_ref().unwrap().placing_book_id, 99511);
    let third = completion(&sync, &pool, 99522).await;
    assert!(!rank.pending_completion_event_ids().await.unwrap().contains(&third));
    assert_eq!(
        rank.accept_completed_chapter(&third).await,
        Err(RankingStoreError::Conflict),
        "busy placement must not acknowledge or lose another book"
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries
         WHERE event_id=$1::uuid AND consumer='ranking-eligibility-v1'",
    )
    .bind(&third)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);

    let question = active.next_question().unwrap().unwrap();
    assert_eq!(
        rank.apply(
            99501,
            active.revision,
            RankingCommand::Answer {
                expected_state_version: question.state_version,
                comparison_id: question.comparison_id,
                outcome: Outcome::Tie,
            },
        )
        .await
        .unwrap()
        .status,
        Some(AnswerStatus::Ranked)
    );
    assert!(rank.pending_completion_event_ids().await.unwrap().contains(&third));
    let other_worker = PgRanking::new(pool.clone());
    let (a, b) = tokio::join!(
        rank.accept_completed_chapter(&third),
        other_worker.accept_completed_chapter(&third)
    );
    assert_eq!(usize::from(a == Ok(true)) + usize::from(b == Ok(true)), 1);
    assert!(a == Ok(false) || b == Ok(false));
    let latest = PgRanking::new(pool.clone()).snapshot(99501).await.unwrap();
    assert_eq!(latest.ranks.len(), 2);
    assert_eq!(latest.placement.unwrap().placing_book_id, 99512);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries
         WHERE consumer='ranking-eligibility-v1'
           AND event_id IN ($1::uuid,$2::uuid,$3::uuid)",
    )
    .bind(&first)
    .bind(&second)
    .bind(&third)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 3);
    let delivered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE id IN ($1::uuid,$2::uuid,$3::uuid)
           AND delivered_at IS NOT NULL",
    )
    .bind(&first)
    .bind(&second)
    .bind(&third)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(delivered, 3);
    let corrupted = "00000000-0000-0000-0000-000000000000";
    assert_eq!(
        rank.accept_completed_chapter(corrupted).await,
        Err(RankingStoreError::Invalid),
        "an event with no C completion is not trusted"
    );

    sqlx::query(
        "DELETE FROM event_deliveries
         WHERE event_id IN ($1::uuid,$2::uuid,$3::uuid)",
    )
    .bind(&first)
    .bind(&second)
    .bind(&third)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM outbox_events
         WHERE id IN ($1::uuid,$2::uuid,$3::uuid)",
    )
    .bind(&first)
    .bind(&second)
    .bind(&third)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM users WHERE id IN (99501,99502)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id IN (99510,99511,99512)")
        .execute(&pool)
        .await
        .unwrap();
}
