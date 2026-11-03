//! AB-08/09: shared PostgreSQL scoped write quota with idempotent retries.
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::repositories::{
    scoped_actions::ScopedActionAdmission,
    PgAccounts,
};

fn digest(label: &str) -> [u8; 32] {
    Sha256::digest(label.as_bytes()).into()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable migrated PostgreSQL tor_ci"]
async fn a09_pg_scoped_comment_budget_is_atomic_across_pools_and_replay_safe() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test DB URL");
    assert!(url.contains("127.0.0.1") && url.ends_with("/tor_ci"));
    let a_pool = sqlx::PgPool::connect(&url).await.unwrap();
    let b_pool = sqlx::PgPool::connect(&url).await.unwrap();
    let a = PgAccounts::from_pool(a_pool.clone());
    let b = PgAccounts::from_pool(b_pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor = a
        .insert_user(
            &format!("ab08-{nonce}@example.invalid"),
            "AB08 Reader",
            "test-only-disabled-password",
        )
        .await
        .unwrap();
    let other = a
        .insert_user(
            &format!("ab08-other-{nonce}@example.invalid"),
            "AB08 Second",
            "test-only-disabled-password",
        )
        .await
        .unwrap();

    // Thirty independent commands on two pools share exactly one fixed
    // 60-second quota: no split-brain limit per process.
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..30 {
        let accounts = if i % 2 == 0 { a.clone() } else { b.clone() };
        let command = digest(&format!("ab08-{nonce}-cmd-{i}"));
        tasks.spawn(async move { accounts.permit_comment_action(actor, &command).await });
    }
    while let Some(task) = tasks.join_next().await {
        assert_eq!(task.unwrap().unwrap(), ScopedActionAdmission::Allowed);
    }

    let blocked = b
        .permit_comment_action(actor, &digest(&format!("ab08-{nonce}-extra")))
        .await
        .unwrap();
    let seconds = match blocked {
        ScopedActionAdmission::Limited { retry_after } => retry_after,
        other => panic!("31st distinct command unexpectedly allowed: {other:?}"),
    };
    assert!((1..=60).contains(&seconds));

    // An HTTP retry of a previously admitted idempotency command is not a
    // new action. The domain layer still checks its exact payload hash.
    assert_eq!(
        b.permit_comment_action(actor, &digest(&format!("ab08-{nonce}-cmd-0")))
            .await
            .unwrap(),
        ScopedActionAdmission::Allowed
    );
    let snapshot: (i32, i64) = sqlx::query_as(
        "SELECT w.admitted,
            (SELECT count(*) FROM scoped_action_command_keys k
             WHERE k.scope=w.scope AND k.actor_id=w.actor_id)
         FROM scoped_action_windows w
         WHERE w.scope='comments' AND w.actor_id=$1",
    )
    .bind(actor)
    .fetch_one(&a_pool)
    .await
    .unwrap();
    assert_eq!(snapshot, (30, 30));
    assert_eq!(
        b.permit_comment_action(other, &digest(&format!("ab08-{nonce}-extra")))
            .await
            .unwrap(),
        ScopedActionAdmission::Allowed
    );

    // A new window resets both the hit count and the replay-key set.
    sqlx::query(
        "UPDATE scoped_action_windows
         SET period_started=clock_timestamp()-INTERVAL '2 minutes'
         WHERE scope='comments' AND actor_id=$1",
    )
    .bind(actor)
    .execute(&a_pool)
    .await
    .unwrap();
    assert_eq!(
        a.permit_comment_action(actor, &digest(&format!("ab08-{nonce}-new")))
            .await
            .unwrap(),
        ScopedActionAdmission::Allowed
    );
    let updated: (i32, i64) = sqlx::query_as(
        "SELECT w.admitted,
            (SELECT count(*) FROM scoped_action_command_keys k
             WHERE k.scope=w.scope AND k.actor_id=w.actor_id)
         FROM scoped_action_windows w
         WHERE w.scope='comments' AND w.actor_id=$1",
    )
    .bind(actor)
    .fetch_one(&b_pool)
    .await
    .unwrap();
    assert_eq!(updated, (1, 1));

    // Security retention removes stale windows and their keys without
    // unbounded scans. The other actor's current window survives.
    sqlx::query(
        "UPDATE scoped_action_windows
         SET touched_at=clock_timestamp()-INTERVAL '2 days'
         WHERE scope='comments' AND actor_id=$1",
    )
    .bind(actor)
    .execute(&a_pool)
    .await
    .unwrap();
    a.prune_security_data().await.unwrap();
    let orphaned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM scoped_action_command_keys
         WHERE scope='comments' AND actor_id=$1",
    )
    .bind(actor)
    .fetch_one(&b_pool)
    .await
    .unwrap();
    assert_eq!(orphaned, 0);
    sqlx::query("DELETE FROM users WHERE id IN ($1,$2)")
        .bind(actor)
        .bind(other)
        .execute(&a_pool)
        .await
        .unwrap();
}
