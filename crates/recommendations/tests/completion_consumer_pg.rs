//! F-05: contract test of verified C completion + private ranking on tor_ci.
//! The actual C-owned event creation is tested separately in C-13 completion_pg;
//! only the fixture here inserts that documented, authoritative shape.
use sqlx::PgPool;
use tor_recommendations::{
    repositories::{PgRanking, RankingCommand, RankingStoreError},
    services::ranking::{AnswerStatus, Outcome},
};

async fn completed_fixture(pool: &PgPool, chapter_id: i64) -> String {
    let book_id = chapter_id - 10;
    let version_id = chapter_id + 10;
    let event_id: String = sqlx::query_scalar(
        "INSERT INTO reader_chapter_completions
          (user_id,book_id,chapter_id,first_version_id,source_event_id)
         VALUES(99501,$1,$2,$3,gen_random_uuid())
         RETURNING source_event_id::text",
    )
    .bind(book_id)
    .bind(chapter_id)
    .bind(version_id)
    .fetch_one(pool)
    .await
    .unwrap();
    // Mirrors C's canonical payload, not an HTTP/client-supplied authority.
    sqlx::query(
        "INSERT INTO outbox_events(
            id,event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers
         ) VALUES(
            $1::uuid,'ChapterCompletedV1',1,'chapter',$2,
            concat('chapter-completed:',99501,':',$2),
            jsonb_build_object(
                'user_id',99501::bigint,'book_id',$3,'chapter_id',$2,
                'version_id',$4,'authority','C-verified-free-position'
            ),
            ARRAY['ranking-eligibility-v1']
         )",
    )
    .bind(&event_id)
    .bind(chapter_id)
    .bind(book_id)
    .bind(version_id)
    .execute(pool)
    .await
    .unwrap();
    event_id
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn f_05_pg_trusted_c_events_are_applied_once_and_wait_for_active_comparison() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never test production");
    let pool = PgPool::connect(&url).await.unwrap();

    // Clean up a previous interrupted test. Always remove receipts first.
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
             VALUES($1,$2,1,'Канонический текст')",
        )
        .bind(99530 + i)
        .bind(99520 + i)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(99520 + i)
            .bind(99530 + i)
            .execute(&pool)
            .await
            .unwrap();
    }

    let ranking = PgRanking::new(pool.clone());
    assert_eq!(ranking.snapshot(99501).await.unwrap().revision, 0);
    assert!(ranking
        .pending_completion_event_ids()
        .await
        .unwrap()
        .is_empty());
    let first = completed_fixture(&pool, 99520).await;
    assert!(ranking
        .pending_completion_event_ids()
        .await
        .unwrap()
        .contains(&first));
    assert!(ranking.accept_completed_chapter(&first).await.unwrap());
    assert!(!ranking.accept_completed_chapter(&first).await.unwrap());
    let snap = PgRanking::new(pool.clone()).snapshot(99501).await.unwrap();
    assert_eq!(snap.ranks.len(), 1);
    assert_eq!(snap.ranks[0].book_id, 99510);
    assert_eq!(snap.revision, 1);
    assert!(ranking.snapshot(99502).await.unwrap().ranks.is_empty());

    let second = completed_fixture(&pool, 99521).await;
    assert!(ranking.accept_completed_chapter(&second).await.unwrap());
    let active = ranking.snapshot(99501).await.unwrap();
    assert_eq!(active.placement.as_ref().unwrap().placing_book_id, 99511);
    let third = completed_fixture(&pool, 99522).await;
    assert!(!ranking
        .pending_completion_event_ids()
        .await
        .unwrap()
        .contains(&third));
    assert_eq!(
        ranking.accept_completed_chapter(&third).await,
        Err(RankingStoreError::Conflict),
        "busy pair must not acknowledge or drop another candidate"
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

    // The third book becomes eligible after the preceding pair is answered.
    let question = active.next_question().unwrap().unwrap();
    assert_eq!(
        ranking
            .apply(
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
    assert!(ranking
        .pending_completion_event_ids()
        .await
        .unwrap()
        .contains(&third));
    let other_worker = PgRanking::new(pool.clone());
    let (a, b) = tokio::join!(
        ranking.accept_completed_chapter(&third),
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
    assert_eq!(
        ranking
            .accept_completed_chapter("00000000-0000-0000-0000-000000000000")
            .await,
        Err(RankingStoreError::Invalid),
        "an event without a C record must never become trusted"
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
