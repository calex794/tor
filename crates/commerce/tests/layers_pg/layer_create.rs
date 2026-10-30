//! D-05: Layer creation has an owner-scoped, database-enforced replay key.
use super::*;

fn create_key(letter: char) -> IdempotencyKey {
    IdempotencyKey::parse(&format!("{}8", letter.to_string().repeat(42))).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_layer_create_is_atomic_replay_safe_and_actor_scoped() {
    let (pool, repo, owner, other, _) = setup("create-idempotent").await;
    let key = create_key('C');
    assert!(
        sqlx::query(
            "INSERT INTO commerce_layers(owner_user_id,title,create_command_title)
             VALUES($1,'Invalid Layer','Unpaired title')"
        )
        .bind(owner)
        .execute(&pool)
        .await
        .is_err(),
        "the SQL CHECK must reject a title without a command key"
    );
    assert!(
        sqlx::query(
            "INSERT INTO commerce_layers(owner_user_id,title,create_command_key)
             VALUES($1,'Invalid Layer',$2)"
        )
        .bind(owner)
        .bind(key.as_str())
        .execute(&pool)
        .await
        .is_err(),
        "the SQL CHECK must reject a command key without a frozen title"
    );
    let first = repo
        .create_layer_once(id(owner), "  My Layer  ", &key)
        .await
        .unwrap();
    assert!(!first.replayed);
    let replay = repo
        .create_layer_once(id(owner), "My Layer", &key)
        .await
        .unwrap();
    assert_eq!(replay.layer_id, first.layer_id);
    assert!(replay.replayed);
    // Renaming display metadata must not invalidate the original command.
    sqlx::query("UPDATE commerce_layers SET title='Renamed Layer' WHERE id=$1")
        .bind(first.layer_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.create_layer_once(id(owner), "My Layer", &key)
            .await
            .unwrap()
            .layer_id,
        first.layer_id
    );
    assert!(
        sqlx::query("UPDATE commerce_layers SET create_command_title='Wrong' WHERE id=$1")
            .bind(first.layer_id)
            .execute(&pool)
            .await
            .is_err(),
        "the original form title is immutable in PostgreSQL"
    );
    assert!(
        sqlx::query("UPDATE commerce_layers SET create_command_key=$2 WHERE id=$1")
            .bind(first.layer_id)
            .bind(create_key('D').as_str())
            .execute(&pool)
            .await
            .is_err(),
        "changing the original command key is forbidden"
    );
    assert_eq!(
        repo.create_layer_once(id(owner), "Changed Layer", &key)
            .await,
        Err(LayerError::ReplayConflict)
    );
    assert_eq!(
        repo.create_layer_once(id(owner), " ", &create_key('D'))
            .await,
        Err(LayerError::Invalid)
    );
    assert_eq!(
        repo.create_layer_once(id(owner), &"д".repeat(121), &create_key('D'))
            .await,
        Err(LayerError::Invalid)
    );

    // Idempotency keys are scoped to the authenticated owner, not global.
    let another = repo
        .create_layer_once(id(other), "My Layer", &key)
        .await
        .unwrap();
    assert_ne!(another.layer_id, first.layer_id);
    assert!(!another.replayed);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layers
         WHERE owner_user_id=$1 AND create_command_key=$2",
    )
    .bind(owner)
    .bind(key.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let (version_count, number): (i64, i64) = sqlx::query_as(
        "SELECT count(*),min(version_no)
         FROM commerce_layer_versions WHERE layer_id=$1",
    )
    .bind(first.layer_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((version_count, number), (1, 1));
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_two_concurrent_creates_have_one_winner_and_one_replay() {
    let (pool, repo, owner, _, _) = setup("create-parallel").await;
    let key = create_key('E');
    let (a, b) = tokio::join!(
        repo.create_layer_once(id(owner), "Atomic Layer", &key),
        repo.create_layer_once(id(owner), "Atomic Layer", &key)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.layer_id, b.layer_id);
    assert_ne!(a.replayed, b.replayed);
    let layer_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layers
         WHERE owner_user_id=$1 AND create_command_key=$2",
    )
    .bind(owner)
    .bind(key.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let versions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commerce_layer_versions WHERE layer_id=$1")
            .bind(a.layer_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let terms: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commerce_layer_terms WHERE layer_id=$1")
            .bind(a.layer_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((layer_count, versions, terms), (1, 1, 0));
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_owner_metadata_hides_restores_without_mutating_paid_versions() {
    let (pool, repo, owner, buyer, book) = setup("metadata-owner").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 250, false).await;
    let paid = repo
        .buy(&cmd(buyer, layer, offer_id, version, 250, 'J'))
        .await
        .unwrap();
    let original = repo
        .owned_layers(id(owner), 100)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.layer_id == layer)
        .unwrap();
    assert_eq!(original.status, "active");
    assert_eq!(original.metadata_revision, 1);
    assert!(repo.owned_layers(id(buyer), 100).await.unwrap().is_empty());

    repo.update_metadata_if_current(
        id(owner),
        id(layer),
        1,
        " Скрытый & слой ",
        " <описание> ",
        "hidden",
    )
    .await
    .unwrap();
    let hidden = repo
        .owned_layers(id(owner), 100)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.layer_id == layer)
        .unwrap();
    assert_eq!(hidden.title, "Скрытый & слой");
    assert_eq!(hidden.description, "<описание>");
    assert_eq!(hidden.status, "hidden");
    assert_eq!(hidden.metadata_revision, 2);
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
    assert_eq!(
        repo.buy(&cmd(buyer, layer, offer_id, version, 250, 'K'))
            .await,
        Err(LayerError::StaleOffer),
        "hidden layer cannot sell new rights"
    );
    assert_eq!(
        repo.update_metadata_if_current(id(owner), id(layer), 1, "Old", "", "active")
            .await,
        Err(LayerError::StaleOffer),
        "a stale form cannot silently restore an old state"
    );
    assert_eq!(
        repo.update_metadata_if_current(id(buyer), id(layer), 2, "Theft", "", "active")
            .await,
        Err(LayerError::Inaccessible),
        "another account cannot edit a layer"
    );
    assert_eq!(
        repo.update_metadata_if_current(id(owner), id(layer), 2, "Bad", "", "blocked")
            .await,
        Err(LayerError::Invalid),
        "the owner is not an operator"
    );
    assert_eq!(
        repo.update_metadata_if_current(id(owner), id(layer), 2, " ", "", "active")
            .await,
        Err(LayerError::Invalid)
    );
    assert_eq!(
        repo.update_metadata_if_current(id(owner), id(layer), 2, "Long", &"д".repeat(2001), "active")
            .await,
        Err(LayerError::Invalid)
    );

    repo.update_metadata_if_current(id(owner), id(layer), 2, "Back", "", "active")
        .await
        .unwrap();
    let restored = repo
        .owned_layers(id(owner), 100)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.layer_id == layer)
        .unwrap();
    assert_eq!(restored.status, "active");
    assert_eq!(restored.metadata_revision, 3);

    sqlx::query("UPDATE commerce_layers SET status='blocked' WHERE id=$1")
        .bind(layer)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.update_metadata_if_current(id(owner), id(layer), 3, "Bypass", "", "active")
            .await,
        Err(LayerError::Inaccessible)
    );
    let blocked = repo
        .owned_layers(id(owner), 100)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.layer_id == layer)
        .unwrap();
    assert_eq!(blocked.status, "blocked");
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
    let (versions, terms, debits): (i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM commerce_layer_versions WHERE layer_id=$1),
            (SELECT count(*) FROM commerce_layer_terms WHERE layer_id=$1),
            (SELECT count(*) FROM reader_wallet_entries WHERE user_id=$2 AND kind='layer_purchase')",
    )
    .bind(layer)
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((versions, terms, debits), (2, 1, 1));
    assert!(paid.term_id > 0);
    pool.close().await;
}
