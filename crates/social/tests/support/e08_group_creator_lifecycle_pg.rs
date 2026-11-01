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
