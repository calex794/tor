//! Real PostgreSQL acceptance: multi-book changes must be one immutable version.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_batch_preview_preserves_paid_history_and_cas() {
    let (pool, repo, owner, buyer, old_book) = setup("batch-preview").await;
    let new_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d05:batch-preview-new','ru','New package book','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (layer, version, offer_id) = offer(&repo, owner, old_book, 200, true).await;
    let receipt = repo
        .buy(&cmd(buyer, layer, offer_id, version, 200, 'K'))
        .await
        .unwrap();
    let before_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commerce_layer_versions WHERE layer_id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    let snapshot = repo
        .preview_composition(id(owner), id(layer), id(version), &[new_book], &[old_book])
        .await
        .unwrap();
    assert_eq!(snapshot.additions.len(), 1);
    assert_eq!(snapshot.removals.len(), 1);
    assert_eq!((snapshot.active_buyers, snapshot.lifetime_buyers), (1, 1));
    assert!(repo.can_read_book(id(buyer), id(old_book)).await.unwrap());
    assert!(!repo.can_read_book(id(buyer), id(new_book)).await.unwrap());
    // Preview never mutates the live version or the purchased wallet entry.
    let same: i64 =
        sqlx::query_scalar("SELECT current_version_id FROM commerce_layers WHERE id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(same, version);
    let new_version = repo
        .apply_composition(
            id(owner),
            id(layer),
            id(version),
            &[new_book],
            &[old_book],
            (snapshot.active_buyers, snapshot.lifetime_buyers),
        )
        .await
        .unwrap();
    let after_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commerce_layer_versions WHERE layer_id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_count,
        before_count + 1,
        "one version for the entire batch"
    );
    let listed: Vec<i64> = sqlx::query_scalar(
        "SELECT book_id FROM commerce_layer_version_books
         WHERE layer_version_id=$1 ORDER BY book_id",
    )
    .bind(new_version)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(listed, vec![new_book]);
    assert!(repo.can_read_book(id(buyer), id(old_book)).await.unwrap());
    assert!(repo.can_read_book(id(buyer), id(new_book)).await.unwrap());
    assert_eq!(
        repo.apply_composition(
            id(owner),
            id(layer),
            id(version),
            &[new_book],
            &[old_book],
            (1, 1),
        )
        .await,
        Err(LayerError::StaleOffer),
    );
    assert_eq!(
        repo.preview_composition(id(buyer), id(layer), id(new_version), &[old_book], &[])
            .await,
        Err(LayerError::Inaccessible),
    );
    let ledger_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_wallet_entries WHERE layer_term_id=$1")
            .bind(receipt.term_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        ledger_count, 1,
        "no purchase or wallet mutation from composition edits"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_batch_rechecks_team_changes_and_rejects_forged_ids() {
    let (pool, repo, owner, collaborator, original) = setup("batch-team").await;
    let additional: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d05:batch-team-new','ru','New team member','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let layer = repo
        .create_layer(id(owner), "Team preflight")
        .await
        .unwrap();
    let version = repo
        .add_book(id(owner), id(layer), id(original))
        .await
        .unwrap();
    assert_eq!(
        repo.preview_composition(
            id(owner),
            id(layer),
            id(version),
            &[additional, additional],
            &[]
        )
        .await,
        Err(LayerError::Invalid),
    );
    let snap = repo
        .preview_composition(id(owner), id(layer), id(version), &[additional], &[])
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',TRUE)",
    )
    .bind(additional)
    .bind(collaborator)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.apply_composition(
            id(owner),
            id(layer),
            id(version),
            &[additional],
            &[],
            (snap.active_buyers, snap.lifetime_buyers),
        )
        .await,
        Err(LayerError::Inaccessible),
    );
    let unchanged: i64 =
        sqlx::query_scalar("SELECT current_version_id FROM commerce_layers WHERE id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unchanged, version);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn h_ui_17_persists_pages_and_applies_one_paid_safe_revision() {
    let (pool, repo, owner, buyer, old_book) = setup("draft-pages").await;
    let a: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d05:draft-a','ru','Draft A','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let b: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d05:draft-b','ru','Draft B','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (layer, version, offer_id) = offer(&repo, owner, old_book, 200, true).await;
    repo.buy(&cmd(buyer, layer, offer_id, version, 200, 'W'))
        .await
        .unwrap();
    let initial = repo.composition_draft(id(owner), id(layer)).await.unwrap();
    assert_eq!((initial.revision, initial.version_id), (0, version));
    let first = repo
        .stage_composition_page(id(owner), id(layer), id(version), 0, &[old_book, a], &[a])
        .await
        .unwrap();
    assert_eq!(first.additions, vec![a]);
    assert_eq!(first.removals, vec![old_book]);
    assert_eq!(
        repo.stage_composition_page(id(owner), id(layer), id(version), 0, &[b], &[b])
            .await,
        Err(LayerError::StaleOffer),
        "an older browser tab must not overwrite this draft",
    );
    let second = repo
        .stage_composition_page(id(owner), id(layer), id(version), first.revision, &[b], &[b])
        .await
        .unwrap();
    assert_eq!(second.additions, vec![a, b]);
    assert_eq!(second.removals, vec![old_book]);
    assert_eq!(
        repo.composition_draft(id(owner), id(layer)).await.unwrap(),
        second,
    );
    assert_eq!(
        repo.composition_draft(id(buyer), id(layer)).await,
        Err(LayerError::Inaccessible),
    );
    assert_eq!(
        repo.stage_composition_page(id(buyer), id(layer), id(version), 0, &[a], &[a])
            .await,
        Err(LayerError::Inaccessible),
    );
    assert_eq!(
        repo.stage_composition_page(
            id(owner),
            id(layer),
            id(version),
            second.revision,
            &[999999999],
            &[999999999],
        )
        .await,
        Err(LayerError::Inaccessible),
    );
    let count_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_versions WHERE layer_id=$1",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(repo.can_read_book(id(buyer), id(old_book)).await.unwrap());
    assert!(!repo.can_read_book(id(buyer), id(a)).await.unwrap());
    let snapshot = repo
        .preview_composition(
            id(owner),
            id(layer),
            id(version),
            &second.additions,
            &second.removals,
        )
        .await
        .unwrap();
    assert_eq!((snapshot.active_buyers, snapshot.lifetime_buyers), (1, 1));
    assert_eq!(
        repo.apply_composition_draft(id(owner), id(layer), id(version), first.revision, (1, 1))
            .await,
        Err(LayerError::StaleOffer),
    );
    assert_eq!(
        repo.apply_composition_draft(id(owner), id(layer), id(version), second.revision, (2, 1))
            .await,
        Err(LayerError::StaleOffer),
    );
    let final_version = repo
        .apply_composition_draft(id(owner), id(layer), id(version), second.revision, (1, 1))
        .await
        .unwrap();
    let count_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_versions WHERE layer_id=$1",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count_after, count_before + 1);
    let selected: Vec<i64> = sqlx::query_scalar(
        "SELECT book_id FROM commerce_layer_version_books
         WHERE layer_version_id=$1 ORDER BY book_id",
    )
    .bind(final_version)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(selected, vec![a, b]);
    assert!(repo.can_read_book(id(buyer), id(old_book)).await.unwrap());
    assert!(repo.can_read_book(id(buyer), id(a)).await.unwrap());
    assert!(repo.can_read_book(id(buyer), id(b)).await.unwrap());
    let after = repo.composition_draft(id(owner), id(layer)).await.unwrap();
    assert_eq!(after.revision, 0, "committed draft is removed");
    assert_eq!(after.version_id, final_version);
    assert_eq!(
        repo.apply_composition_draft(id(owner), id(layer), id(version), second.revision, (1, 1))
            .await,
        Err(LayerError::StaleOffer),
    );
    pool.close().await;
}
