//! F-05: real PostgreSQL tests must only run on disposable tor_ci.
use sqlx::PgPool;
use tor_recommendations::{
    repositories::{PgRanking, RankingCommand, RankingStoreError},
    services::ranking::{AnswerStatus, Outcome, RankingSnapshot},
};

async fn fixture(name: &str) -> (PgRanking, PgPool, i64, [i64; 3]) {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PG");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = PgPool::connect(&url).await.unwrap();
    let email = format!("f05-ranking-{name}@tor-ci.example");
    sqlx::query("DELETE FROM users WHERE email_normalized=$1")
        .bind(&email)
        .execute(&pool)
        .await
        .unwrap();
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'F05 Reader','$argon2id$test') RETURNING id",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut ids = [0_i64; 3];
    for (index, id) in ids.iter_mut().enumerate() {
        *id = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES($1,'ru',$2,'public')
             ON CONFLICT(source_identity,target_language)
             DO UPDATE SET title=EXCLUDED.title,visibility='public' RETURNING id",
        )
        .bind(format!("ci:f05:ranking:{name}:{index}"))
        .bind(format!("Книга {index}"))
        .fetch_one(&pool)
        .await
        .unwrap();
    }
    (PgRanking::new(pool.clone()), pool, uid, ids)
}

async fn seeded(repo: &PgRanking, uid: i64, ids: &[i64; 3]) -> RankingSnapshot {
    let first = repo
        .apply(uid, 0, RankingCommand::StartVerified(ids[0]))
        .await
        .unwrap();
    assert_eq!(first.status, Some(AnswerStatus::Ranked));
    let second = repo
        .apply(
            uid,
            first.snapshot.revision,
            RankingCommand::StartVerified(ids[1]),
        )
        .await
        .unwrap();
    assert_eq!(second.status, Some(AnswerStatus::Placing));
    second.snapshot
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn f_05_pg_question_answer_is_durable_and_audit_once() {
    let (repo, pool, uid, ids) = fixture("durable").await;
    let before = seeded(&repo, uid, &ids).await;
    let q = before.next_question().unwrap().unwrap();
    let done = repo
        .apply(
            uid,
            before.revision,
            RankingCommand::Answer {
                expected_state_version: q.state_version,
                comparison_id: q.comparison_id,
                outcome: Outcome::AWins,
            },
        )
        .await
        .unwrap();
    assert_eq!(done.status, Some(AnswerStatus::Ranked));
    assert_eq!(done.snapshot.ranks.len(), 2);
    assert_eq!(
        PgRanking::new(pool.clone()).snapshot(uid).await.unwrap(),
        done.snapshot
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_book_comparison WHERE user_id=$1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn f_05_pg_skip_is_sticky_and_explicit_resume_fences_old_question() {
    let (repo, pool, uid, ids) = fixture("skip").await;
    let before = seeded(&repo, uid, &ids).await;
    let q = before.next_question().unwrap().unwrap();
    let skip = repo
        .apply(
            uid,
            before.revision,
            RankingCommand::Answer {
                expected_state_version: q.state_version,
                comparison_id: q.comparison_id.clone(),
                outcome: Outcome::Skip,
            },
        )
        .await
        .unwrap();
    assert_eq!(skip.status, Some(AnswerStatus::Skipped));
    let loaded = PgRanking::new(pool.clone()).snapshot(uid).await.unwrap();
    assert_eq!(loaded, skip.snapshot);
    assert!(loaded.next_question().unwrap().is_none());
    assert_eq!(
        repo.apply(uid, before.revision, RankingCommand::ResumeExhausted)
            .await
            .unwrap_err(),
        RankingStoreError::Stale
    );
    let resume = repo
        .apply(uid, loaded.revision, RankingCommand::ResumeExhausted)
        .await
        .unwrap();
    let new_q = resume.snapshot.next_question().unwrap().unwrap();
    assert_ne!(new_q.comparison_id, q.comparison_id);
    assert_eq!(
        repo.apply(
            uid,
            resume.snapshot.revision,
            RankingCommand::Answer {
                expected_state_version: q.state_version,
                comparison_id: q.comparison_id,
                outcome: Outcome::AWins,
            }
        )
        .await
        .unwrap_err(),
        RankingStoreError::Stale
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn f_05_pg_parallel_devices_commit_only_one_comparison() {
    let (repo, pool, uid, ids) = fixture("two-devices").await;
    let before = seeded(&repo, uid, &ids).await;
    let q = before.next_question().unwrap().unwrap();
    let cmd = RankingCommand::Answer {
        expected_state_version: q.state_version,
        comparison_id: q.comparison_id,
        outcome: Outcome::Tie,
    };
    let (a, b) = tokio::join!(
        repo.apply(uid, before.revision, cmd.clone()),
        repo.apply(uid, before.revision, cmd),
    );
    let results = [a, b];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(RankingStoreError::Stale)))
            .count(),
        1
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_book_comparison WHERE user_id=$1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert_eq!(repo.snapshot(uid).await.unwrap().ranks.len(), 2);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn f_05_pg_defer_survives_restart_and_does_not_auto_release() {
    let (repo, pool, uid, ids) = fixture("defer").await;
    let before = seeded(&repo, uid, &ids).await;
    let q = before.next_question().unwrap().unwrap();
    let outcome = if q.book_a_id == ids[1] {
        Outcome::DeferA
    } else {
        Outcome::DeferB
    };
    let saved = repo
        .apply(
            uid,
            before.revision,
            RankingCommand::Answer {
                expected_state_version: q.state_version,
                comparison_id: q.comparison_id,
                outcome,
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.status, Some(AnswerStatus::Deferred(ids[1])));
    let reloaded = PgRanking::new(pool.clone()).snapshot(uid).await.unwrap();
    assert!(reloaded.deferred_books.contains(&ids[1]));
    assert_eq!(
        repo.apply(
            uid,
            reloaded.revision,
            RankingCommand::StartVerified(ids[1])
        )
        .await
        .unwrap_err(),
        RankingStoreError::Invalid
    );
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('f05-ranking-other@tor-ci.example','Other Reader','$argon2id$test')
         ON CONFLICT(email_normalized)
         DO UPDATE SET display_name=EXCLUDED.display_name RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let their = repo.snapshot(other).await.unwrap();
    assert!(their.ranks.is_empty());
    assert!(their.deferred_books.is_empty());
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn f_05_pg_bad_question_rolls_back_state_and_event() {
    let (repo, pool, uid, ids) = fixture("rollback").await;
    let before = seeded(&repo, uid, &ids).await;
    let q = before.next_question().unwrap().unwrap();
    assert_eq!(
        repo.apply(
            uid,
            before.revision,
            RankingCommand::Answer {
                expected_state_version: q.state_version,
                comparison_id: "wrong-question".into(),
                outcome: Outcome::BWins,
            }
        )
        .await
        .unwrap_err(),
        RankingStoreError::Stale
    );
    assert_eq!(repo.snapshot(uid).await.unwrap(), before);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_book_comparison WHERE user_id=$1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn f_05_pg_visible_ranked_list_excludes_blocked_books_and_unrelated_users() {
    let (repo, pool, uid, ids) = fixture("ranked-list").await;
    let before = seeded(&repo, uid, &ids).await;
    let question = before.next_question().unwrap().unwrap();
    let placed = repo.apply(
        uid,
        before.revision,
        RankingCommand::Answer {
            expected_state_version: question.state_version,
            comparison_id: question.comparison_id,
            outcome: Outcome::Tie,
        },
    ).await.unwrap();
    assert_eq!(placed.status, Some(AnswerStatus::Ranked));
    let ranked = repo.visible_ranks(uid).await.unwrap();
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].tier, ranked[1].tier);
    assert_eq!(
        ranked.iter().map(|row| row.book_id).collect::<std::collections::BTreeSet<_>>(),
        [ids[0], ids[1]].into_iter().collect()
    );
    assert!(ranked.iter().all(|row| row.title.starts_with("Книга ")));

    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(ids[0]).execute(&pool).await.unwrap();
    let after_hold = repo.visible_ranks(uid).await.unwrap();
    assert_eq!(after_hold.len(), 1);
    assert_eq!(after_hold[0].book_id, ids[1]);
    assert_eq!(repo.visible_ranks(0).await.unwrap_err(), RankingStoreError::Invalid);
    assert!(repo.visible_ranks(i64::MAX).await.unwrap().is_empty());
}
