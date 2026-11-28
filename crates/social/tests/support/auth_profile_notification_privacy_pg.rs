/// Legacy mailbox text can remain in notification snapshots after the account
/// has been repaired. It is not authoritative identity or a redirect target.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn auth_profile_03b_legacy_notification_mailbox_is_never_returned() {
    let (pool, users) = fixture().await;
    let (sender, recipient, outsider) = (users[0], users[1], users[2]);
    let safe_name = format!("Safe Reader {sender}");
    sqlx::query("UPDATE users SET display_name=$2 WHERE id=$1")
        .bind(sender)
        .bind(&safe_name)
        .execute(&pool)
        .await
        .unwrap();
    let mail = MailService::new(PgMail::new(pool.clone()));
    let friends = PgFriendships::new(pool.clone());
    let notifications = PgNotifications::new(pool.clone());
    let conversation = mail.start(sender, recipient).await.unwrap();
    mail.send(sender, conversation, "PRIVATE MESSAGE", &key(91))
        .await
        .unwrap();
    friends.request(sender, recipient).await.unwrap();
    for _ in 0..12 {
        if notifications.deliver_pending(100).await.unwrap() < 100 {
            break;
        }
    }
    let before = notifications.list(recipient, None, 50).await.unwrap();
    assert_eq!(before.len(), 2);
    let stale = "old-mailbox@example.invalid";
    sqlx::query(
        "UPDATE user_notifications SET body=$2,url=$3
         WHERE recipient_user_id=$1",
    )
    .bind(recipient)
    .bind(format!("{stale} написал вам"))
    .bind("/users/old-mailbox%40example.invalid")
    .execute(&pool)
    .await
    .unwrap();
    let repaired = notifications.list(recipient, None, 50).await.unwrap();
    assert_eq!(repaired, before);
    for row in &repaired {
        assert!(!row.body.contains(stale));
        assert!(!row.url.contains("%40"));
        assert!(!row.is_read);
    }
    assert!(notifications.list(outsider, None, 50).await.unwrap().is_empty());
    assert!(!notifications.mark_read(outsider, repaired[0].id).await.unwrap());
    assert!(notifications.mark_read(recipient, repaired[0].id).await.unwrap());
    let after = notifications.list(recipient, None, 50).await.unwrap();
    assert!(after[0].is_read);

    // A first handle claim and rename update the old friendship target,
    // without changing event identity, recipient, ordering or read state.
    let handle = format!("notice{sender}");
    sqlx::query("INSERT INTO account_settings(user_id,handle) VALUES($1,$2)")
        .bind(sender)
        .bind(&handle)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET display_name='Renamed reader' WHERE id=$1")
        .bind(sender)
        .execute(&pool)
        .await
        .unwrap();
    let stable = format!("/users/by-handle/{handle}");
    let rows = notifications.list(recipient, None, 50).await.unwrap();
    let old_friend = after.iter().find(|row| row.kind == "friendship").unwrap();
    let friend = rows.iter().find(|row| row.kind == "friendship").unwrap();
    assert_eq!(friend.id, old_friend.id);
    assert_eq!(friend.is_read, old_friend.is_read);
    assert_eq!(friend.url, stable);
    assert!(friend.body.contains("Renamed reader"));

    friends.request(sender, outsider).await.unwrap();
    for _ in 0..12 {
        if notifications.deliver_pending(100).await.unwrap() < 100 {
            break;
        }
    }
    let fresh = notifications.list(outsider, None, 50).await.unwrap();
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].url, stable);
    let persisted_url: String = sqlx::query_scalar("SELECT url FROM user_notifications WHERE id=$1")
        .bind(fresh[0].id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted_url, stable);
    cleanup(&pool, &users).await;
}

/// Read-only inventory must distinguish matching canonical events from
/// marker-bearing rows with inconsistent bindings, without changing either.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_auth_profile_06a_historical_marker_audit_is_bounded_and_read_only() {
    let (pool, users) = fixture().await;
    let (sender, recipient, outsider) = (users[0], users[1], users[2]);
    let mail = MailService::new(PgMail::new(pool.clone()));
    let friends = PgFriendships::new(pool.clone());
    let notifications = PgNotifications::new(pool.clone());
    let conversation = mail.start(sender, recipient).await.unwrap();
    mail.send(sender, conversation, "AUDIT FIXTURE", &key(92))
        .await
        .unwrap();
    friends.request(sender, recipient).await.unwrap();
    for _ in 0..12 {
        if notifications.deliver_pending(100).await.unwrap() < 100 {
            break;
        }
    }
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM user_notifications WHERE recipient_user_id=$1 ORDER BY id",
    )
    .bind(recipient)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(ids.len(), 2);
    let through = notifications
        .legacy_notification_audit_high_watermark()
        .await
        .unwrap();
    assert!(through >= ids[1]);

    let safe = notifications
        .audit_legacy_notification_batch(ids[0] - 1, ids[0], 1)
        .await
        .unwrap();
    assert_eq!(safe.last_scanned_id, Some(ids[0]));
    assert_eq!(
        (safe.scanned, safe.marked, safe.actor_event_matched),
        (1, 0, 0)
    );

    sqlx::query(
        "UPDATE user_notifications
         SET body='historical@example.invalid',url='/users/historical%40example.invalid'
         WHERE id=ANY($1)",
    )
    .bind(&ids)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE user_notifications SET actor_user_id=$2 WHERE id=$1")
        .bind(ids[1])
        .bind(outsider)
        .execute(&pool)
        .await
        .unwrap();
    let first = notifications
        .audit_legacy_notification_batch(ids[0] - 1, ids[1], 1)
        .await
        .unwrap();
    assert_eq!(first.last_scanned_id, Some(ids[0]));
    assert_eq!(
        (first.scanned, first.marked, first.actor_event_matched),
        (1, 1, 1)
    );
    let second = notifications
        .audit_legacy_notification_batch(ids[1] - 1, ids[1], 1)
        .await
        .unwrap();
    assert_eq!(second.last_scanned_id, Some(ids[1]));
    assert_eq!(
        (second.scanned, second.marked, second.actor_event_matched),
        (1, 1, 0)
    );
    let empty = notifications
        .audit_legacy_notification_batch(ids[1], ids[1], 1)
        .await
        .unwrap();
    assert_eq!(empty.last_scanned_id, None);
    assert_eq!(
        (empty.scanned, empty.marked, empty.actor_event_matched),
        (0, 0, 0)
    );
    for (after, upper, limit) in [
        (-1, through, 1),
        (through, 0, 1),
        (0, through, 0),
        (0, through, 1001),
    ] {
        assert!(matches!(
            notifications
                .audit_legacy_notification_batch(after, upper, limit)
                .await,
            Err(MailError::Invalid)
        ));
    }
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_notifications
         WHERE id=ANY($1) AND body='historical@example.invalid'
           AND url='/users/historical%40example.invalid' AND read_at IS NULL",
    )
    .bind(&ids)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, 2);
    cleanup(&pool, &users).await;
}
