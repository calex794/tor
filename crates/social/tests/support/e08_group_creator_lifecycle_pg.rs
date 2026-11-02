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
    // A delivered but now revoked offer is retained for audit, not displayed.
    assert!(notifications.list(recipient, None, 50).await.unwrap().is_empty());
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

/// If delivery has already read the source event and is blocked on the group,
/// a concurrent revoked offer must not become a late bell notification.
/// Rechecking the transfer in a fresh snapshot after the group lock is key.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_transfer_notice_rechecks_offer_after_concurrent_revocation() {
    let (pool, users) = fixture().await;
    let (owner, recipient) = (users[0], users[1]);
    let groups = PgGroupMail::new(pool.clone());
    let notifications = PgNotifications::new(pool.clone());
    // Existing unrelated events cannot be mistaken for this offer.
    for _ in 0..10 {
        if notifications.deliver_pending(100).await.unwrap() < 100 {
            break;
        }
    }
    let group_id = groups.create(owner, &[recipient], "Race-safe transfer").await.unwrap();
    let offer_id = groups.propose_transfer(owner, group_id, recipient).await.unwrap();
    let event_id: String = sqlx::query_scalar(
        "SELECT id::text FROM outbox_events
         WHERE event_type='social.group_creator_transfer.proposed'
           AND aggregate_id=$1",
    )
    .bind(offer_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    // The source transaction owns the group before notification delivery
    // starts. The consumer must block on it, after claiming the outbox event.
    let mut owner_tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE")
        .bind(group_id)
        .execute(&mut *owner_tx)
        .await
        .unwrap();
    let worker_repo = notifications.clone();
    let worker = tokio::spawn(async move { worker_repo.deliver_pending(1).await });

    // Query own database sessions to synchronize the race without a fragile
    // sleep; both old joined-lock and new group-first code wait on this row.
    let start = std::time::Instant::now();
    let mut waiting_for_group = false;
    while start.elapsed() < std::time::Duration::from_secs(10) {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE pid<>pg_backend_pid() AND usename=current_user
               AND datname=current_database() AND state='active'
               AND wait_event_type='Lock'
               AND query LIKE '%social_group_conversations%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if waiting > 0 {
            waiting_for_group = true;
            break;
        }
        if worker.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(waiting_for_group, "notification worker must reach the held group");
    let revoked = sqlx::query(
        "UPDATE social_group_creator_transfers
         SET state='revoked',resolved_at=clock_timestamp()
         WHERE id=$1 AND state='pending'",
    )
    .bind(offer_id)
    .execute(&mut *owner_tx)
    .await
    .unwrap();
    assert_eq!(revoked.rows_affected(), 1);
    owner_tx.commit().await.unwrap();

    assert_eq!(worker.await.unwrap(), Ok(1));
    assert!(notifications.list(recipient, None, 50).await.unwrap().is_empty());
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries
         WHERE consumer='user_notifications' AND event_id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 1, "a suppressed event is still acknowledged once");
    assert_eq!(notifications.deliver_pending(1).await.unwrap(), 0);

    sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
        .bind(group_id)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &users).await;
}
/// Already delivered creator-offer bells disappear as soon as the offer is
/// no longer actionable. Filtering happens before LIMIT; an invalid newest
/// offer must not hide an older valid one in the notification inbox.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_delivered_transfer_bell_only_shows_actionable_offers() {
    let (pool, users) = fixture().await;
    let (owner, recipient) = (users[0], users[1]);
    let groups = PgGroupMail::new(pool.clone());
    let notifications = PgNotifications::new(pool.clone());
    let live_group = groups
        .create(owner, &[recipient], "Live transfer offer")
        .await
        .unwrap();
    let live_offer = groups
        .propose_transfer(owner, live_group, recipient)
        .await
        .unwrap();
    for _ in 0..10 {
        if notifications.deliver_pending(100).await.unwrap() < 100 {
            break;
        }
    }
    let live = notifications.list(recipient, None, 1).await.unwrap();
    assert_eq!(live.len(), 1);
    let live_id = live[0].id;
    assert!(live_offer > 0);
    assert_eq!(
        notifications
            .list_section(recipient, None, 50, "system")
            .await
            .unwrap()
            .len(),
        1
    );

    for resolution in ["revoked", "declined", "expired", "accepted", "left"] {
        let group_id = groups
            .create(owner, &[recipient], "Transient transfer offer")
            .await
            .unwrap();
        let offer_id = groups
            .propose_transfer(owner, group_id, recipient)
            .await
            .unwrap();
        for _ in 0..10 {
            if notifications.deliver_pending(100).await.unwrap() < 100 {
                break;
            }
        }
        let before = notifications.list(recipient, None, 2).await.unwrap();
        assert_eq!(before.len(), 2, "a live offer must initially appear: {resolution}");
        let stale_id = before[0].id;
        assert!(stale_id > live_id);
        assert_eq!(before[1].id, live_id);

        match resolution {
            "revoked" => groups.revoke_transfer(owner, group_id, offer_id).await.unwrap(),
            "declined" => groups.decline_transfer(recipient, offer_id).await.unwrap(),
            "expired" => {
                sqlx::query(
                    "UPDATE social_group_creator_transfers
                     SET created_at=clock_timestamp()-interval '10 days',
                         expires_at=clock_timestamp()-interval '3 days'
                     WHERE id=$1",
                )
                .bind(offer_id)
                .execute(&pool)
                .await
                .unwrap();
            }
            "accepted" => assert_eq!(
                groups.accept_transfer(recipient, offer_id).await,
                Ok(group_id)
            ),
            "left" => groups.leave(recipient, group_id).await.unwrap(),
            _ => unreachable!(),
        }

        let visible = notifications.list(recipient, None, 1).await.unwrap();
        assert_eq!(visible.len(), 1, "stale offer was rendered: {resolution}");
        assert_eq!(visible[0].id, live_id, "filter before LIMIT: {resolution}");
        assert_eq!(
            notifications.list(recipient, Some(stale_id + 1), 1)
                .await.unwrap()[0].id,
            live_id,
            "cursor must skip stale rows: {resolution}"
        );
        assert_eq!(
            notifications
                .list_section(recipient, None, 50, "system")
                .await
                .unwrap()
                .len(),
            1,
            "system tab must not expose stale offers: {resolution}"
        );
        assert!(
            !notifications.mark_read(recipient, stale_id).await.unwrap(),
            "an old Open form must return not found: {resolution}"
        );
        let stored: i64 =
            sqlx::query_scalar("SELECT count(*) FROM user_notifications WHERE id=$1")
                .bind(stale_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, 1, "do not delete the historical projection");
        sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
            .bind(group_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    assert!(notifications.mark_read(recipient, live_id).await.unwrap());
    assert!(notifications.list(recipient, None, 1).await.unwrap()[0].is_read);
    sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
        .bind(live_group)
        .execute(&pool)
        .await
        .unwrap();
    assert!(notifications.list(recipient, None, 50).await.unwrap().is_empty());
    assert!(!notifications.mark_read(recipient, live_id).await.unwrap());
    cleanup(&pool, &users).await;
}
