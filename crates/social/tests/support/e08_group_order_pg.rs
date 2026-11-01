// Pause an actual sender after message allocation at the outbox INSERT.
// Another participant must wait for that sender, not publish a later ID first.
async fn e08_wait_for_blocker(pool: &PgPool, waiting: i32, blocker: i32) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let blocked: bool = sqlx::query_scalar("SELECT $2=ANY(pg_blocking_pids($1))")
            .bind(waiting)
            .bind(blocker)
            .fetch_one(pool)
            .await
            .unwrap();
        if blocked {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_group_mail_commit_order_preserves_pagination() {
    let (pool, users) = fixture().await;
    let (a, b, c) = (users[0], users[1], users[2]);
    let groups = PgGroupMail::new(pool.clone());
    let group = groups.create(a, &[b, c], "Ordered messages").await.unwrap();
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let first_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let second_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&first_pool)
        .await
        .unwrap();
    let second_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&second_pool)
        .await
        .unwrap();
    let mut pause = pool.begin().await.unwrap();
    let pause_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *pause)
        .await
        .unwrap();
    sqlx::query("LOCK TABLE outbox_events IN SHARE MODE")
        .execute(&mut *pause)
        .await
        .unwrap();

    let first_store = PgGroupMail::new(first_pool.clone());
    let first =
        tokio::spawn(async move { first_store.send(a, group, "First commit", &key(481)).await });
    let first_paused = e08_wait_for_blocker(&pool, first_pid, pause_pid).await;
    let second_store = PgGroupMail::new(second_pool.clone());
    let second = tokio::spawn(async move {
        second_store
            .send(b, group, "Second commit", &key(482))
            .await
    });
    let ordered = e08_wait_for_blocker(&pool, second_pid, first_pid).await;
    // Always release our barrier before asserting, including on regression.
    pause.rollback().await.unwrap();
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert!(first_paused, "first sender must reach the outbox barrier");
    assert!(
        ordered,
        "different senders must serialize before allocating IDs"
    );
    assert!(first.id < second.id);
    let newest = groups.messages(c, group, None, 1).await.unwrap();
    assert_eq!(newest[0].id, second.id);
    let older = groups
        .messages(c, group, Some(newest[0].id), 1)
        .await
        .unwrap();
    assert_eq!(older[0].id, first.id);
    assert!(groups
        .messages(c, group, Some(first.id), 1)
        .await
        .unwrap()
        .is_empty());
    assert!(
        groups
            .send(a, group, "First commit", &key(481))
            .await
            .unwrap()
            .replayed
    );
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE event_type='social.group_mail.created' AND aggregate_id=ANY($1::bigint[])",
    )
    .bind(vec![first.id, second.id])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        events, 4,
        "two recipients per message, replay creates no event"
    );
    first_pool.close().await;
    second_pool.close().await;
    cleanup(&pool, &users).await;
}
