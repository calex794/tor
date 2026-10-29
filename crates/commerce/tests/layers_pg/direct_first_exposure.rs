//! C/D-06: a real direct purchase has at most one first protected grant.
use super::*;
use tor_commerce::{
    domain::money::{CheckoutRepository, PurchaseCommand},
    repositories::{
        checkout::PgCheckout,
        direct_exposure::{DirectGrantKind, PgDirectFirstExposure},
    },
};
use tor_contracts::paid_delivery::{ChapterVersionRef, DeliveryError};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_pg_direct_first_grant_cas_owner_pinned_v1_and_revocation() {
    let (pool, _layers, owner, buyer, book) = setup("direct-grant-cas").await;
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Direct V1 exposure','public','paid',75) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Private chapter V1') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    let purchased = PgCheckout::new(pool.clone())
        .checkout_once(
            &PurchaseCommand::new(
                id(buyer),
                WalletId::new(buyer).unwrap(),
                ChapterVersionRef {
                    chapter_id: id(chapter),
                    version_id: id(version),
                },
                MoneyMinor::new(75).unwrap(),
                IdempotencyKey::parse(&format!("{}8", "Q".repeat(42))).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let purchase = purchased.purchase_id.get();
    let gate = PgDirectFirstExposure::new(pool.clone());
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_purchase_first_exposures WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, 0, "checkout alone does not read paid content");
    assert_eq!(
        gate.confirm_rendered(id(buyer), id(chapter), id(version), id(purchase))
            .await,
        Err(DeliveryError::Denied),
        "no browser render can precede a signed grant"
    );
    assert_eq!(
        gate.record(
            id(owner),
            id(chapter),
            id(version),
            Some(id(purchase)),
            DirectGrantKind::Chapter
        )
        .await,
        Err(DeliveryError::Denied)
    );
    assert_eq!(
        gate.record(
            id(buyer),
            id(chapter),
            id(i64::MAX),
            Some(id(purchase)),
            DirectGrantKind::Chapter
        )
        .await,
        Err(DeliveryError::Denied)
    );
    assert_eq!(
        gate.record(
            id(buyer),
            id(chapter),
            id(version),
            Some(id(i64::MAX)),
            DirectGrantKind::Chapter
        )
        .await,
        Err(DeliveryError::Denied)
    );
    let (explicit, unsourced) = tokio::join!(
        gate.record(
            id(buyer),
            id(chapter),
            id(version),
            Some(id(purchase)),
            DirectGrantKind::Chapter
        ),
        gate.record(
            id(buyer),
            id(chapter),
            id(version),
            None,
            DirectGrantKind::Asset
        )
    );
    assert_eq!(explicit, Ok(()));
    assert_eq!(unsourced, Ok(()));
    let first: (i64, i64, i64, i64, String, String) = sqlx::query_as(
        "SELECT purchase_id,user_id,chapter_id,version_id,object_kind,granted_at::TEXT
         FROM chapter_purchase_first_exposures WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (first.0, first.1, first.2, first.3),
        (purchase, buyer, chapter, version)
    );
    assert!(first.4 == "chapter" || first.4 == "asset");
    gate.record(
        id(buyer),
        id(chapter),
        id(version),
        None,
        DirectGrantKind::Chapter,
    )
    .await
    .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_purchase_first_exposures WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let unchanged: (String, String) = sqlx::query_as(
        "SELECT object_kind,granted_at::TEXT FROM chapter_purchase_first_exposures
         WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unchanged, (first.4, first.5));
    assert!(sqlx::query(
        "UPDATE chapter_purchase_first_exposures
             SET granted_at=clock_timestamp() WHERE purchase_id=$1",
    )
    .bind(purchase)
    .execute(&pool)
    .await
    .is_err());
    // Browser confirmation is independent of grant and exactly-once.
    let empty: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_purchase_render_confirmations WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        empty, 0,
        "credential issuance does not imply chapter rendering"
    );
    let (first_ack, replay_ack) = tokio::join!(
        gate.confirm_rendered(id(buyer), id(chapter), id(version), id(purchase)),
        gate.confirm_rendered(id(buyer), id(chapter), id(version), id(purchase)),
    );
    assert_eq!(first_ack, Ok(()));
    assert_eq!(replay_ack, Ok(()));
    assert_eq!(
        gate.confirm_rendered(id(owner), id(chapter), id(version), id(purchase))
            .await,
        Err(DeliveryError::Denied),
    );
    assert_eq!(
        gate.confirm_rendered(id(buyer), id(chapter), id(i64::MAX), id(purchase))
            .await,
        Err(DeliveryError::Denied),
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_purchase_render_confirmations WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert!(
        sqlx::query("DELETE FROM chapter_purchase_render_confirmations WHERE purchase_id=$1",)
            .bind(purchase)
            .execute(&pool)
            .await
            .is_err(),
        "render evidence is append-only"
    );
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    let debits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (balance, debits),
        (1425, 1),
        "first grants do not move funds"
    );
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,'ci:direct-first-revoked','refund')",
    )
    .bind(purchase)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        gate.record(
            id(buyer),
            id(chapter),
            id(version),
            None,
            DirectGrantKind::Chapter
        )
        .await,
        Err(DeliveryError::Denied)
    );
    pool.close().await;
}
