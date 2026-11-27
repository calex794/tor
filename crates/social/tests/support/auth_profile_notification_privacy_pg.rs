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
    cleanup(&pool, &users).await;
}
