//! C/D-06: real PostgreSQL grant-first-exposure CAS and term isolation.
use super::*;
use tor_commerce::repositories::layer_exposure::{LayerGrantKind, PgLayerFirstExposure};
use tor_contracts::paid_delivery::DeliveryError;

async fn paid_version(pool: &PgPool, book: i64) -> (i64, i64) {
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Paid exposure chapter','public','paid',75) RETURNING id",
    )
    .bind(book)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Protected content') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    (chapter, version)
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_pg_layer_first_grant_is_once_only_and_does_not_create_money() {
    let (pool, repo, owner, buyer, book) = setup("first-exposure-once").await;
    let (layer, version, offered) = offer(&repo, owner, book, 225, false).await;
    let (chapter, published) = paid_version(&pool, book).await;
    let paid = repo
        .buy(&cmd(buyer, layer, offered, version, 225, 'X'))
        .await
        .unwrap();
    let barrier = PgLayerFirstExposure::new(pool.clone());

    let initial: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_term_first_exposures WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        initial, 0,
        "checkout and reader shell do not consume content"
    );
    assert_eq!(
        barrier
            .record(
                id(owner),
                id(chapter),
                id(published),
                LayerGrantKind::Chapter
            )
            .await,
        Err(DeliveryError::Denied),
        "a foreign actor must not synthesize an exposure"
    );
    assert_eq!(
        barrier
            .record(
                id(buyer),
                id(chapter),
                id(i64::MAX),
                LayerGrantKind::Chapter
            )
            .await,
        Err(DeliveryError::Denied),
        "a browser-chosen/unpublished edition must not pass"
    );
    let (a, b) = tokio::join!(
        barrier.record(
            id(buyer),
            id(chapter),
            id(published),
            LayerGrantKind::Chapter
        ),
        barrier.record(id(buyer), id(chapter), id(published), LayerGrantKind::Asset)
    );
    assert_eq!(a, Ok(()));
    assert_eq!(b, Ok(()));
    let evidence: (i64, i64, i64, String) = sqlx::query_as(
        "SELECT term_id,chapter_id,version_id,object_kind
         FROM commerce_layer_term_first_exposures WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(evidence.0, paid.term_id);
    assert_eq!((evidence.1, evidence.2), (chapter, published));
    assert!(evidence.3 == "chapter" || evidence.3 == "asset");
    let started: String = sqlx::query_scalar(
        "SELECT granted_at::TEXT FROM commerce_layer_term_first_exposures WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    barrier
        .record(
            id(buyer),
            id(chapter),
            id(published),
            LayerGrantKind::Chapter,
        )
        .await
        .unwrap();
    let after: (i64, String) = sqlx::query_as(
        "SELECT count(*), min(granted_at)::TEXT
         FROM commerce_layer_term_first_exposures WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, (1, started));

    // A signed grant is not a browser render: no confirmation until ACK.
    let no_open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_term_render_confirmations WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(no_open, 0);
    let (first_ack, replay_ack) = tokio::join!(
        barrier.confirm_rendered(id(buyer), id(chapter), id(published)),
        barrier.confirm_rendered(id(buyer), id(chapter), id(published)),
    );
    assert_eq!(first_ack, Ok(()));
    assert_eq!(replay_ack, Ok(()));
    assert_eq!(
        barrier.confirm_rendered(id(owner), id(chapter), id(published)).await,
        Err(DeliveryError::Denied),
    );
    let confirmed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_term_render_confirmations WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(confirmed, 1);
    assert!(sqlx::query(
        "UPDATE commerce_layer_term_render_confirmations SET confirmed_at=now() WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .execute(&pool)
    .await
    .is_err(), "first browser render confirmation is immutable");

    let mutating = sqlx::query(
        "UPDATE commerce_layer_term_first_exposures
         SET object_kind='asset' WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .execute(&pool)
    .await;
    assert!(mutating.is_err(), "first exposure is immutable");

    let (wallet, debit, no_refund_policy): (i64, i64, bool) = sqlx::query_as(
        "SELECT
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1),
           (SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1),
           (SELECT refund_policy_version IS NULL AND refund_deadline_at IS NULL
            FROM commerce_layer_terms WHERE id=$2)",
    )
    .bind(buyer)
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((wallet, debit, no_refund_policy), (1275, 1, true));

    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block','c06-paid-exposure-revoked')",
    )
    .bind(paid.term_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        barrier
            .record(
                id(buyer),
                id(chapter),
                id(published),
                LayerGrantKind::Chapter
            )
            .await,
        Err(DeliveryError::Denied)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_pg_future_renewal_is_not_marked_by_current_term_grant() {
    let (pool, repo, owner, buyer, book) = setup("first-exposure-future-term").await;
    let (layer, version, offered) = offer(&repo, owner, book, 200, false).await;
    let (chapter, published) = paid_version(&pool, book).await;
    let current = repo
        .buy(&cmd(buyer, layer, offered, version, 200, 'Y'))
        .await
        .unwrap();
    let future = repo
        .buy(&cmd(buyer, layer, offered, version, 200, 'Z'))
        .await
        .unwrap();
    assert_ne!(current.term_id, future.term_id);
    let barrier = PgLayerFirstExposure::new(pool.clone());
    barrier
        .record(
            id(buyer),
            id(chapter),
            id(published),
            LayerGrantKind::Chapter,
        )
        .await
        .unwrap();
    barrier.confirm_rendered(id(buyer), id(chapter), id(published)).await.unwrap();
    let active_open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_term_render_confirmations WHERE term_id=$1",
    ).bind(current.term_id).fetch_one(&pool).await.unwrap();
    let future_open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_term_render_confirmations WHERE term_id=$1",
    ).bind(future.term_id).fetch_one(&pool).await.unwrap();
    assert_eq!((active_open, future_open), (1, 0), "future P2 is not opened by P1");
    let first: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM commerce_layer_term_first_exposures
         WHERE term_id=$1",
    )
    .bind(current.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let next: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM commerce_layer_term_first_exposures
         WHERE term_id=$1",
    )
    .bind(future.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (first, next),
        (1, 0),
        "future paid P2 must not inherit P1 exposure"
    );
    pool.close().await;
}
