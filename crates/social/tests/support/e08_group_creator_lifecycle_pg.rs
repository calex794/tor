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
    groups
        .transfer_creator(founder, group_id, successor)
        .await
        .unwrap();
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

    groups
        .transfer_creator(founder, group_id, successor)
        .await
        .unwrap();
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
        groups.transfer_creator(successor, group_id, other).await,
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
