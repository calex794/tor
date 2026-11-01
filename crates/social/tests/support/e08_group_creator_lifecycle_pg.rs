async fn consented_handoff(store: &PgGroupMail, owner: i64, group: i64, recipient: i64) {
    let offer = store.propose_transfer(owner, group, recipient).await.unwrap();
    assert!(store.get(owner, group).await.unwrap().is_creator);
    assert!(!store.get(recipient, group).await.unwrap().is_creator);
    assert_eq!(store.accept_transfer(recipient, offer).await, Ok(group));
    assert_eq!(store.accept_transfer(recipient, offer).await, Err(MailError::NotFound));
}

/// E-08: losing the historical founder after an explicit transfer must
/// not cascade-delete a group controlled by a different active participant.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_group_survives_founder_deletion_after_creator_transfer() {
    let (pool, users) = fixture().await;
    let (founder, successor, observer) = (users[0], users[1], users[2]);
    let groups = PgGroupMail::new(pool.clone());
    let group_id = groups
        .create(founder, &[successor, observer], "Surviving group")
        .await
        .unwrap();

    assert!(groups.get(founder, group_id).await.unwrap().is_creator);
    assert!(!groups.get(successor, group_id).await.unwrap().is_creator);
    consented_handoff(&groups, founder, group_id, successor).await;
    groups.leave(founder, group_id).await.unwrap();
    assert!(groups.get(successor, group_id).await.unwrap().is_creator);

    // This is a real FK cascade in a disposable database, not a mocked
    // repository operation or a manual deletion of the group's founder field.
    let deleted = sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(founder)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(deleted.rows_affected(), 1);

    let founder_provenance: Option<i64> =
        sqlx::query_scalar("SELECT created_by FROM social_group_conversations WHERE id=$1")
            .bind(group_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(founder_provenance, None);
    let surviving = groups.get(successor, group_id).await.unwrap();
    assert_eq!(surviving.title, "Surviving group");
    assert!(surviving.is_creator);
    assert_eq!(groups.members(successor, group_id).await.unwrap().len(), 2);
    assert_eq!(groups.list(observer, None, 10).await.unwrap()[0].id, group_id);

    let sent = groups
        .send(successor, group_id, "The group is still active", &key(971))
        .await
        .unwrap();
    let messages = groups.messages(observer, group_id, None, 10).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, sent.id);
    assert_eq!(messages[0].body, "The group is still active");

    sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
        .bind(group_id)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &[successor, observer]).await;
}

/// Closing or deleting the *active* owner cannot silently orphan a group.
/// Only a deliberate transfer permits account closure or hard deletion.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_group_creator_account_close_and_deletion_require_explicit_handoff() {
    let (pool, users) = fixture().await;
    let (founder, successor, other) = (users[0], users[1], users[2]);
    let groups = PgGroupMail::new(pool.clone());
    let group_id = groups
        .create(founder, &[successor, other], "Requires handoff")
        .await
        .unwrap();

    for sql in [
        "DELETE FROM users WHERE id=$1",
        "UPDATE users SET account_state='closed' WHERE id=$1",
    ] {
        let denied = sqlx::query(sql)
            .bind(founder)
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            denied.as_database_error().and_then(|error| error.code()).as_deref(),
            Some("23514"),
            "active creator cannot disappear: {sql}"
        );
    }
    assert!(groups.get(founder, group_id).await.unwrap().is_creator);
    assert_eq!(groups.members(founder, group_id).await.unwrap().len(), 3);

    consented_handoff(&groups, founder, group_id, successor).await;
    for sql in [
        "DELETE FROM users WHERE id=$1",
        "UPDATE users SET account_state='closed' WHERE id=$1",
    ] {
        let denied = sqlx::query(sql)
            .bind(successor)
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            denied.as_database_error().and_then(|error| error.code()).as_deref(),
            Some("23514"),
            "role transfer must also protect the new creator: {sql}"
        );
    }
    // An already-closed member cannot become the next creator.
    sqlx::query("UPDATE users SET account_state='closed' WHERE id=$1")
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        groups.propose_transfer(successor, group_id, other).await,
        Err(MailError::NotFound)
    );

    // The original creator is now free to close the account, but the group
    // remains owned by the active successor.
    sqlx::query("UPDATE users SET account_state='closed' WHERE id=$1")
        .bind(founder)
        .execute(&pool)
        .await
        .unwrap();
    assert!(groups.get(successor, group_id).await.unwrap().is_creator);
    assert_eq!(groups.get(other, group_id).await.unwrap().id, group_id);
    sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
        .bind(group_id)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &users).await;
}

/// Explicit disband permits solo creators to close accounts without
/// appointing a new owner. Other active members prevent destructive removal.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_group_disband_requires_sole_active_creator() {
    let (pool, users) = fixture().await;
    let (creator, peer, outsider) = (users[0], users[1], users[2]);
    let store = PgGroupMail::new(pool.clone());
    let id = store
        .create(creator, &[peer], "Temporary group")
        .await
        .unwrap();
    store
        .send(creator, id, "Soon deleted", &key(972))
        .await
        .unwrap();
    assert_eq!(store.disband(outsider, id).await, Err(MailError::NotFound));
    assert_eq!(store.disband(peer, id).await, Err(MailError::NotFound));
    assert_eq!(store.disband(creator, id).await, Err(MailError::Conflict));
    assert!(store.get(peer, id).await.is_ok());
    store.leave(peer, id).await.unwrap();
    assert_eq!(store.disband(creator, id).await, Ok(()));
    assert_eq!(store.disband(creator, id).await, Err(MailError::NotFound));
    assert_eq!(store.get(creator, id).await, Err(MailError::NotFound));
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM social_group_messages WHERE group_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    sqlx::query("UPDATE users SET account_state='closed' WHERE id=$1")
        .bind(creator)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &users).await;
}


/// The recipient, never the sender, controls acceptance. A stale, revoked,
/// declined or replayed proposal cannot change the creator role.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_creator_handoff_requires_consent_and_is_single_use() {
    let (pool, users) = fixture().await;
    let (owner, recipient, stranger) = (users[0], users[1], users[2]);
    let groups = PgGroupMail::new(pool.clone());
    let id = groups.create(owner, &[recipient], "Consented handoff").await.unwrap();
    let first = groups.propose_transfer(owner, id, recipient).await.unwrap();
    assert!(groups.get(owner, id).await.unwrap().is_creator);
    assert!(!groups.get(recipient, id).await.unwrap().is_creator);
    assert_eq!(groups.leave(owner, id).await, Err(MailError::Conflict));
    assert_eq!(
        groups.propose_transfer(owner, id, recipient).await,
        Err(MailError::Conflict)
    );
    assert_eq!(groups.accept_transfer(stranger, first).await, Err(MailError::NotFound));
    assert_eq!(groups.decline_transfer(stranger, first).await, Err(MailError::NotFound));
    assert_eq!(groups.revoke_transfer(stranger, id, first).await, Err(MailError::NotFound));

    groups.decline_transfer(recipient, first).await.unwrap();
    assert_eq!(groups.accept_transfer(recipient, first).await, Err(MailError::NotFound));
    let second = groups.propose_transfer(owner, id, recipient).await.unwrap();
    assert!(second > first);
    groups.revoke_transfer(owner, id, second).await.unwrap();
    assert_eq!(groups.accept_transfer(recipient, second).await, Err(MailError::NotFound));
    assert_eq!(groups.revoke_transfer(owner, id, second).await, Err(MailError::NotFound));

    let third = groups.propose_transfer(owner, id, recipient).await.unwrap();
    let (a, b) = tokio::join!(
        groups.accept_transfer(recipient, third),
        groups.accept_transfer(recipient, third)
    );
    assert!(a.is_ok() != b.is_ok(), "only one acceptance can succeed");
    assert!(groups.get(recipient, id).await.unwrap().is_creator);
    assert!(!groups.get(owner, id).await.unwrap().is_creator);
    assert_eq!(groups.accept_transfer(recipient, third).await, Err(MailError::NotFound));
    assert_eq!(groups.propose_transfer(owner, id, stranger).await, Err(MailError::NotFound));
    assert_eq!(groups.revoke_transfer(owner, id, third).await, Err(MailError::NotFound));

    sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &users).await;
}

/// A member who leaves or an offer that expires cannot be made creator.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_creator_handoff_rechecks_membership_and_expiration() {
    let (pool, users) = fixture().await;
    let (owner, recipient) = (users[0], users[1]);
    let groups = PgGroupMail::new(pool.clone());
    let id = groups.create(owner, &[recipient], "Stale offers").await.unwrap();
    let first = groups.propose_transfer(owner, id, recipient).await.unwrap();
    groups.leave(recipient, id).await.unwrap();
    assert_eq!(groups.accept_transfer(recipient, first).await, Err(MailError::NotFound));
    groups.revoke_transfer(owner, id, first).await.unwrap();

    // Recipient explicitly rejoins through a fresh membership invitation.
    let invite = groups.invite(owner, id, recipient).await.unwrap();
    groups.accept_invite(recipient, invite).await.unwrap();
    let second = groups.propose_transfer(owner, id, recipient).await.unwrap();
    sqlx::query(
        "UPDATE social_group_creator_transfers
         SET created_at=clock_timestamp()-interval '10 days',
             expires_at=clock_timestamp()-interval '3 days'
         WHERE id=$1",
    )
    .bind(second)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(groups.accept_transfer(recipient, second).await, Err(MailError::NotFound));
    let third = groups.propose_transfer(owner, id, recipient).await.unwrap();
    assert!(third > second);
    let expired: String = sqlx::query_scalar(
        "SELECT state FROM social_group_creator_transfers WHERE id=$1",
    )
    .bind(second)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(expired, "expired");
    groups.revoke_transfer(owner, id, third).await.unwrap();
    assert!(groups.get(owner, id).await.unwrap().is_creator);
    sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &users).await;
}

/// An ownership offer must reach the addressee, not every group member.
/// Rejected, revoked, expired and departed-member offers must not produce
/// late notifications; source events are acknowledged exactly once.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_creator_transfer_bell_notifies_only_for_live_proposals() {
    let (pool, users) = fixture().await;
    let (owner, recipient, other) = (users[0], users[1], users[2]);
    let groups = PgGroupMail::new(pool.clone());
    let notifications = PgNotifications::new(pool.clone());
    let id = groups.create(owner, &[recipient, other], "Transfer alert").await.unwrap();

    let first = groups.propose_transfer(owner, id, recipient).await.unwrap();
    let source_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE event_type='social.group_creator_transfer.proposed' AND aggregate_id=$1",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source_count, 1);
    for _ in 0..10 {
        if notifications.deliver_pending(100).await.unwrap() < 100 {
            break;
        }
    }
    let inbox = notifications.list(recipient, None, 50).await.unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].kind, "mail");
    assert_eq!(inbox[0].url, "/mail/group-transfers");
    assert!(inbox[0].title.contains("управлять группой"));
    assert!(!inbox[0].body.contains("Transfer alert"));
    assert!(notifications.list(owner, None, 50).await.unwrap().is_empty());
    assert!(notifications.list(other, None, 50).await.unwrap().is_empty());
    assert_eq!(notifications.deliver_pending(100).await.unwrap(), 0);

    let declined = groups.propose_transfer(owner, id, other).await.unwrap_err();
    assert_eq!(declined, MailError::Conflict, "only one offer at a time");
    groups.revoke_transfer(owner, id, first).await.unwrap();
    let second = groups.propose_transfer(owner, id, recipient).await.unwrap();
    groups.decline_transfer(recipient, second).await.unwrap();
    let third = groups.propose_transfer(owner, id, recipient).await.unwrap();
    sqlx::query(
        "UPDATE social_group_creator_transfers
         SET created_at=clock_timestamp()-interval '10 days',
             expires_at=clock_timestamp()-interval '3 days'
         WHERE id=$1",
    )
    .bind(third)
    .execute(&pool)
    .await
    .unwrap();
    let fourth = groups.propose_transfer(owner, id, recipient).await.unwrap();
    groups.leave(recipient, id).await.unwrap();
    for _ in 0..10 {
        if notifications.deliver_pending(100).await.unwrap() < 100 {
            break;
        }
    }
    assert_eq!(notifications.list(recipient, None, 50).await.unwrap().len(), 1);
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries d
         JOIN outbox_events e ON e.id=d.event_id
         WHERE d.consumer='user_notifications'
           AND e.event_type='social.group_creator_transfer.proposed'
           AND e.aggregate_id=ANY($1)",
    )
    .bind(vec![first, second, third, fourth])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt_count, 4);
    sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &users).await;
}
