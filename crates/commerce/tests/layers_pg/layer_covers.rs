//! Package image moderation cannot touch immutable paid composition.
use super::*;
use tor_commerce::repositories::layer_covers::{PackageCoverUpload, PgLayerCovers};

fn upload<'a>(card_key: &'a str, main_key: &'a str) -> PackageCoverUpload<'a> {
    PackageCoverUpload {
        card_key,
        main_key,
        card_sha256: [9; 32],
        main_sha256: [8; 32],
        card_bytes: 128,
        main_bytes: 256,
    }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn h_ui_18_owner_private_cover_moderation_keeps_paid_rights() {
    let (pool, layers, owner, moderator, book) = setup("package-cover").await;
    let (layer, old_version, _offer_id) = offer(&layers, owner, book, 200, true).await;
    let repo = PgLayerCovers::new(pool.clone());
    let (approved, pending) = repo.owner_view(id(owner), id(layer)).await.unwrap();
    assert!(approved.is_none() && pending.is_none());
    assert!(!repo.owner_can_upload(id(moderator), id(layer)).await.unwrap());

    let card = format!("staging/covers/variants/packages/{layer}/a/card.webp");
    let main = format!("staging/covers/variants/packages/{layer}/a/main.webp");
    let first = repo.reserve(id(owner), id(layer), &upload(&card, &main)).await.unwrap();
    assert!(repo.public_current(id(layer)).await.unwrap().is_none());
    assert_eq!(
        repo.reserve(id(owner), id(layer), &upload(&card, &main)).await,
        Err(LayerError::StaleOffer),
    );
    assert_eq!(
        repo.reserve(id(moderator), id(layer), &upload(&card, &main)).await,
        Err(LayerError::Inaccessible),
    );
    repo.mark_pending(id(owner), id(first)).await.unwrap();
    assert_eq!(repo.pending_queue(40).await.unwrap().iter()
        .filter(|row| row.candidate.id == first).count(), 1);
    assert_eq!(
        repo.review(id(owner), id(first), true, "").await,
        Err(LayerError::Inaccessible),
    );
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'moderator',TRUE)",
    )
    .bind(moderator)
    .execute(&pool)
    .await
    .unwrap();
    repo.review(id(moderator), id(first), true, "").await.unwrap();
    assert_eq!(
        repo.review(id(moderator), id(first), false, "late").await,
        Err(LayerError::StaleOffer),
    );
    let (approved, pending) = repo.owner_view(id(owner), id(layer)).await.unwrap();
    assert_eq!(approved.unwrap().id, first);
    assert!(pending.is_none());
    assert_eq!(repo.public_current(id(layer)).await.unwrap().unwrap().id, first);
    sqlx::query("UPDATE commerce_layers SET status='hidden' WHERE id=$1")
        .bind(layer).execute(&pool).await.unwrap();
    assert!(repo.public_current(id(layer)).await.unwrap().is_none(),
        "a hidden package cannot expose its approved artwork");
    sqlx::query("UPDATE commerce_layers SET status='active' WHERE id=$1")
        .bind(layer).execute(&pool).await.unwrap();

    let card_b = format!("staging/covers/variants/packages/{layer}/b/card.webp");
    let main_b = format!("staging/covers/variants/packages/{layer}/b/main.webp");
    let second = repo.reserve(id(owner), id(layer), &upload(&card_b, &main_b)).await.unwrap();
    repo.mark_pending(id(owner), id(second)).await.unwrap();
    assert_eq!(
        repo.review(id(moderator), id(second), false, "").await,
        Err(LayerError::Invalid),
    );
    repo.review(id(moderator), id(second), false, "Не подходит").await.unwrap();
    assert_eq!(repo.owner_view(id(owner), id(layer)).await.unwrap().0.unwrap().id, first);

    let card_c = format!("staging/covers/variants/packages/{layer}/c/card.webp");
    let main_c = format!("staging/covers/variants/packages/{layer}/c/main.webp");
    let third = repo.reserve(id(owner), id(layer), &upload(&card_c, &main_c)).await.unwrap();
    repo.mark_pending(id(owner), id(third)).await.unwrap();
    repo.review(id(moderator), id(third), true, "").await.unwrap();
    assert_eq!(repo.owner_view(id(owner), id(layer)).await.unwrap().0.unwrap().id, third);
    let superseded: String =
        sqlx::query_scalar("SELECT status FROM commerce_layer_cover_candidates WHERE id=$1")
            .bind(first)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(superseded, "superseded");

    let current: i64 = sqlx::query_scalar(
        "SELECT current_version_id FROM commerce_layers WHERE id=$1",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(current, old_version, "cover change never modifies paid composition");
    pool.close().await;
}
