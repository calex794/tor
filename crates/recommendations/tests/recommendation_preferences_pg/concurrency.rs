use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn two_adapters_serialize_explicit_replay_and_cas() {
    let (pool, store, actor, book) = fixture("concurrency").await;
    let other = PgRecommendations::new(pool.clone());
    let hide = new_ticket().unwrap();
    let (left, right) = tokio::join!(
        store.set_feedback(actor, book, Choice::NotInterested, 0, &hide),
        other.set_feedback(actor, book, Choice::NotInterested, 0, &hide)
    );
    let receipt = left.unwrap();
    assert_eq!(receipt, right.unwrap());
    assert_eq!(counts(&pool, actor).await, (1, 1, 1, 0));
    assert_eq!(preference(&pool, actor, book).await.unwrap().1, 1);

    let left_key = new_ticket().unwrap();
    let right_key = new_ticket().unwrap();
    let (left, right) = tokio::join!(
        store.set_feedback(actor, book, Choice::Clear, 1, &left_key),
        other.set_feedback(actor, book, Choice::Clear, 1, &right_key)
    );
    let outcomes = [left, right];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(FeedbackError::Conflict)))
            .count(),
        1
    );
    assert_eq!(counts(&pool, actor).await, (1, 1, 2, 0));
    assert_eq!(preference(&pool, actor, book).await.unwrap().1, 2);

    assert_eq!(counts(&pool, actor).await.3, 0);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn expiry_and_explicit_clear_cannot_lose_each_others_cas() {
    let (pool, store, actor, book) = fixture("retention-race").await;
    store
        .set_feedback(
            actor,
            book,
            Choice::NotInterested,
            0,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE rec_book_preferences SET expires_at=now()-interval '2 hours'
         WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let other = PgRecommendations::new(pool.clone());
    let key = new_ticket().unwrap();
    let (pruned, cleared) = tokio::join!(
        store.prune_preferences(),
        other.set_feedback(actor, book, Choice::Clear, 1, &key)
    );
    pruned.unwrap();
    match cleared {
        Ok(receipt) => {
            assert_eq!(receipt.revision, 2);
            assert_eq!(epoch(&pool, actor).await, 0);
            let current = preference(&pool, actor, book).await.unwrap();
            assert_eq!((current.0, current.1), (false, 2));
            let future: bool = sqlx::query_scalar(
                "SELECT expires_at>now()+interval '29 days' FROM rec_book_preferences
                 WHERE user_id=$1 AND book_id=$2",
            )
            .bind(actor)
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(future, "only the accepted explicit command renews TTL");
        }
        Err(FeedbackError::Conflict) => {
            assert_eq!(epoch(&pool, actor).await, 1);
            assert!(preference(&pool, actor, book).await.is_none());
        }
        other => panic!("unexpected retention race result: {other:?}"),
    }
    assert_eq!(counts(&pool, actor).await.3, 0);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn retention_also_erases_inactive_accounts_without_collecting() {
    let (pool, store, actor, book) = fixture("inactive-ttl").await;
    store
        .set_feedback(
            actor,
            book,
            Choice::NotInterested,
            0,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE rec_book_preferences SET expires_at=now()-interval '2 hours'
         WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE rec_user_preference_commands SET created_at=now()-interval '31 days'
         WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    store.prune_preferences().await.unwrap();
    assert_eq!(counts(&pool, actor).await, (1, 0, 0, 0));
    assert_eq!(epoch(&pool, actor).await, 1);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn retention_skips_a_locked_prefix_and_busy_user_before_applying_the_limit() {
    let (pool, store, _actor, book) = fixture("locked-prefix").await;
    let mut actors = Vec::new();
    for index in 0..34 {
        let email = format!("explicit-pref-lock-{index}@tor-ci.example");
        sqlx::query("DELETE FROM users WHERE email_normalized=$1")
            .bind(&email)
            .execute(&pool)
            .await
            .unwrap();
        let actor: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'Preference lock batch','$argon2id$test') RETURNING id",
        )
        .bind(email)
        .fetch_one(&pool)
        .await
        .unwrap();
        store
            .set_feedback(
                actor,
                book,
                Choice::NotInterested,
                0,
                &new_ticket().unwrap(),
            )
            .await
            .unwrap();
        actors.push(actor);
    }
    sqlx::query(
        "UPDATE rec_book_preferences SET expires_at=now()-interval '2 hours'
         WHERE user_id=ANY($1::bigint[])",
    )
    .bind(&actors)
    .execute(&pool)
    .await
    .unwrap();

    let held = actors[..32].to_vec();
    let mut heads = pool.begin().await.unwrap();
    let locked: Vec<i64> = sqlx::query_scalar(
        "SELECT user_id FROM rec_preference_accounts
         WHERE user_id=ANY($1::bigint[]) ORDER BY user_id FOR UPDATE",
    )
    .bind(&held)
    .fetch_all(&mut *heads)
    .await
    .unwrap();
    assert_eq!(locked.len(), 32);
    let mut user = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(actors[32])
        .execute(&mut *user)
        .await
        .unwrap();

    // Neither the 32 setting locks nor the A-side transition may consume all
    // work slots or make the cleaner wait before reaching the free account.
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), store.prune_preferences())
        .await
        .expect("retention must not wait for either row lock");
    result.unwrap();
    assert!(preference(&pool, actors[33], book).await.is_none());
    assert_eq!(epoch(&pool, actors[33]).await, 1);
    for actor in &actors[..33] {
        assert!(preference(&pool, *actor, book).await.is_some());
        assert_eq!(epoch(&pool, *actor).await, 0);
    }

    user.rollback().await.unwrap();
    heads.rollback().await.unwrap();
    // Drain only through the real bounded adapter; never narrow its SQL to
    // fixture identities. Assertions remain scoped to our own accounts.
    for _ in 0..2 {
        store.prune_preferences().await.unwrap();
    }
    for actor in &actors {
        assert!(preference(&pool, *actor, book).await.is_none());
        assert_eq!(epoch(&pool, *actor).await, 1);
    }
}
