/// Canonical public-room behavior mirrors NEwBOO join/leave/send/read.
/// Tests run only on a disposable /tor_ci PostgreSQL with real SQL grants.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_book_rooms_canonical_join_rejoin_visibility_and_retry() {
    let (pool, users) = fixture().await;
    let (a, b, outsider) = (users[0], users[1], users[2]);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let first: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','Book room public','public') RETURNING id",
    )
    .bind(format!("ci:e08:room:public:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let draft: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','Book room hidden','draft') RETURNING id",
    )
    .bind(format!("ci:e08:room:draft:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let held: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,admin_hold)
         VALUES($1,'ru','Book room held','public',true) RETURNING id",
    )
    .bind(format!("ci:e08:room:held:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','Second public room','public') RETURNING id",
    )
    .bind(format!("ci:e08:room:other:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let rooms = PgBookRooms::new(pool.clone());

    assert_eq!(rooms.join(a, 0).await, Err(MailError::Invalid));
    assert_eq!(rooms.join(a, draft).await, Err(MailError::NotFound));
    assert_eq!(rooms.join(a, held).await, Err(MailError::NotFound));
    assert_eq!(rooms.join(a, i64::MAX).await, Err(MailError::NotFound));
    assert_eq!(rooms.messages(a, first, None, 50).await, Err(MailError::NotFound));
    assert_eq!(rooms.send(a, first, "No membership", &key(812)).await, Err(MailError::NotFound));
    rooms.join(a, first).await.unwrap();
    rooms.join(a, first).await.unwrap();
    rooms.join(b, first).await.unwrap();
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM social_book_room_members
         WHERE book_id=$1 AND status='active'",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 2);
    assert_eq!(rooms.messages(outsider, first, None, 50).await, Err(MailError::NotFound));
    assert_eq!(rooms.send(outsider, first, "Forged", &key(813)).await, Err(MailError::NotFound));

    let one = rooms.send(a, first, "  Hello book  ", &key(814)).await.unwrap();
    assert!(!one.replayed);
    let replay = rooms.send(a, first, "Hello book", &key(814)).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(one.id, replay.id);
    assert_eq!(rooms.send(a, first, "Changed", &key(814)).await, Err(MailError::Conflict));
    rooms.join(a, other).await.unwrap();
    assert_eq!(rooms.send(a, other, "Hello book", &key(814)).await, Err(MailError::Conflict));
    let two = rooms.send(b, first, "B reply", &key(815)).await.unwrap();
    let messages = rooms.messages(a, first, None, 50).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].id, one.id);
    assert_eq!(messages[1].id, two.id);
    assert_eq!(messages[0].sender_name, "Mail fixture");
    assert_eq!(rooms.messages(a, first, Some(two.id), 1).await.unwrap()[0].id, one.id);

    let forged = sqlx::query(
        "INSERT INTO social_book_room_messages(book_id,sender_id,body,command_key)
         VALUES($1,$2,'forged',$3)",
    )
    .bind(first)
    .bind(outsider)
    .bind(key(816))
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        forged.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    assert!(
        sqlx::query("UPDATE social_book_room_messages SET body='changed' WHERE id=$1")
            .bind(one.id)
            .execute(&pool)
            .await
            .is_err()
    );

    rooms.leave(b, first).await.unwrap();
    assert_eq!(rooms.messages(b, first, None, 50).await, Err(MailError::NotFound));
    assert_eq!(rooms.send(b, first, "Exited", &key(817)).await, Err(MailError::NotFound));
    rooms.join(b, first).await.unwrap();
    assert_eq!(rooms.messages(b, first, None, 50).await.unwrap().len(), 2);
    let canonical: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM social_book_rooms WHERE book_id=$1",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(canonical, 1, "rejoin reuses the original public book room");

    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rooms.join(outsider, first).await, Err(MailError::NotFound));
    assert_eq!(rooms.messages(a, first, None, 50).await, Err(MailError::NotFound));
    assert_eq!(rooms.send(a, first, "Hidden", &key(818)).await, Err(MailError::NotFound));
    rooms.leave(b, first).await.unwrap(); // leaving does not disclose content
    sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    rooms.join(b, first).await.unwrap();
    assert_eq!(rooms.messages(b, first, None, 50).await.unwrap().len(), 2);
    cleanup(&pool, &users).await;
    // Books and audit history are intentionally left for disposal of tor_ci.
}

/// The repository's FOR SHARE/FOR UPDATE fences must hold against concurrent
/// membership departure and a Book visibility hold, not just sequential checks.
/// A postponed sender must never persist its text after either ACL change.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_book_room_send_cannot_race_leave_or_moderation_hold() {
    let (pool, users) = fixture().await;
    let actor = users[0];
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','Race-tested book room','public') RETURNING id",
    )
    .bind(format!("ci:e08:room:race:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let rooms = PgBookRooms::new(pool.clone());
    rooms.join(actor, book_id).await.unwrap();

    // Lock actor membership just before an exit commits.
    let mut leave_tx = pool.begin().await.unwrap();
    let status: String = sqlx::query_scalar(
        "SELECT status FROM social_book_room_members
         WHERE book_id=$1 AND user_id=$2 FOR UPDATE",
    )
    .bind(book_id)
    .bind(actor)
    .fetch_one(&mut *leave_tx)
    .await
    .unwrap();
    assert_eq!(status, "active");
    let sending = {
        let rooms = rooms.clone();
        tokio::spawn(async move {
            rooms
                .send(actor, book_id, "After leave", &key(820))
                .await
        })
    };
    tokio::task::yield_now().await;
    sqlx::query(
        "UPDATE social_book_room_members
         SET status='left',left_at=clock_timestamp()
         WHERE book_id=$1 AND user_id=$2",
    )
    .bind(book_id)
    .bind(actor)
    .execute(&mut *leave_tx)
    .await
    .unwrap();
    leave_tx.commit().await.unwrap();
    assert_eq!(sending.await.unwrap(), Err(MailError::NotFound));
    assert_eq!(
        rooms.messages(actor, book_id, None, 50).await,
        Err(MailError::NotFound)
    );

    // The reader explicitly rejoins, then a later admin hold races a send.
    rooms.join(actor, book_id).await.unwrap();
    let mut hold_tx = pool.begin().await.unwrap();
    let locked: i64 =
        sqlx::query_scalar("SELECT id FROM books WHERE id=$1 FOR UPDATE")
            .bind(book_id)
            .fetch_one(&mut *hold_tx)
            .await
            .unwrap();
    assert_eq!(locked, book_id);
    let sending = {
        let rooms = rooms.clone();
        tokio::spawn(async move {
            rooms
                .send(actor, book_id, "After hold", &key(821))
                .await
        })
    };
    tokio::task::yield_now().await;
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&mut *hold_tx)
        .await
        .unwrap();
    hold_tx.commit().await.unwrap();
    assert_eq!(sending.await.unwrap(), Err(MailError::NotFound));
    assert_eq!(rooms.join(actor, book_id).await, Err(MailError::NotFound));
    assert_eq!(
        rooms.messages(actor, book_id, None, 50).await,
        Err(MailError::NotFound)
    );
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM social_book_room_messages WHERE book_id=$1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, 0, "no message may cross either ACL transition");

    cleanup(&pool, &users).await;
    // Book fixture remains in disposable tor_ci for database teardown.
}
