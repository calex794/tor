//! Cross-page draft: serializable CAS, no paid-version writes before confirmation.
use super::*;
use tor_commerce::repositories::layer_drafts::LayerStagePage;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_selection_draft_persists_across_search_and_rejects_stale_versions() {
    let (pool, repo, owner, outsider, book) = setup("draft-cross-page").await;
    let layer = repo
        .create_layer(id(owner), "Draft selection")
        .await
        .unwrap();
    let first = repo.selection_draft(id(owner), id(layer)).await.unwrap();
    assert_eq!(first.revision, 0);
    let page = repo
        .composition_tiles(id(owner), id(layer), None, "", 50)
        .await
        .unwrap();
    assert_eq!(
        page.books.iter().map(|tile| tile.id).collect::<Vec<_>>(),
        vec![book]
    );
    let staged = repo
        .stage_selection_page(
            id(owner),
            id(layer),
            &LayerStagePage {
                version: id(first.base_version_id),
                revision: 0,
                after: None,
                query: String::new(),
                visible: vec![book],
                selected: vec![book],
            },
        )
        .await
        .unwrap()
        .0;
    assert_eq!(
        (staged.revision, staged.additions.as_slice()),
        (1, &[book][..])
    );
    let unchanged: i64 =
        sqlx::query_scalar("SELECT current_version_id FROM commerce_layers WHERE id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unchanged, first.base_version_id);
    let searched = repo
        .stage_selection_page(
            id(owner),
            id(layer),
            &LayerStagePage {
                version: id(first.base_version_id),
                revision: 1,
                after: None,
                query: "nothing-matches".into(),
                visible: vec![],
                selected: vec![],
            },
        )
        .await
        .unwrap()
        .0;
    assert_eq!(searched.revision, 2);
    assert_eq!(searched.additions, vec![book]);
    assert_eq!(
        repo.stage_selection_page(
            id(owner),
            id(layer),
            &LayerStagePage {
                version: id(first.base_version_id),
                revision: 1,
                after: None,
                query: String::new(),
                visible: vec![book],
                selected: vec![],
            }
        )
        .await
        .err(),
        Some(LayerError::StaleOffer),
    );
    assert_eq!(
        repo.selection_draft(id(outsider), id(layer)).await.err(),
        Some(LayerError::Inaccessible),
    );
    // A separate older per-book editor can advance the live version.
    let next = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let stale = repo.selection_draft(id(owner), id(layer)).await.unwrap();
    assert_eq!(stale.base_version_id, first.base_version_id);
    assert_ne!(stale.base_version_id, next);
    repo.clear_selection_draft(id(owner), id(layer), id(stale.base_version_id), 2)
        .await
        .unwrap();
    let reset = repo.selection_draft(id(owner), id(layer)).await.unwrap();
    assert_eq!((reset.base_version_id, reset.revision), (next, 0));
    pool.close().await;
}
