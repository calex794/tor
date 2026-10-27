//! S034 (internal): buyer pays pinned V1, C saves/restores position,
//! and revocation, moderation and storefront hiding agree across D and C.
//! This test never connects to a production database or external payment service.
use sqlx::PgPool;
use tor_commerce::{
    domain::money::{
        CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
    },
    repositories::{checkout::PgCheckout, entitlements::PgDirectEntitlements},
};
use tor_contracts::paid_delivery::{
    Actor, ChapterVersionRef, DenialReason, EntitlementDecision, EntitlementPort, PositiveId,
};
use tor_reading::{
    repositories::PgReadingSync,
    services::{
        progress::{reader_blocks, valid_text_place, ReadingPlace},
        sync::SyncError,
    },
};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL with all SQLx migrations"]
async fn s034_checkout_paid_v1_restore_position_and_enforce_revocation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG required");
    assert!(url.ends_with("/tor_ci"), "never connect to a real wallet");
    let pool = PgPool::connect(&url).await.unwrap();

    let buyer: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s034-integrated-buyer@example.test','Paid buyer','$argon2id$ci') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,500,'RUB')",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:s034:integrated:20261027','ru','Книга с покупкой','public')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Платная глава','public','paid',150) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'А😀Б

Продолжение') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();

    let actor = Actor {
        user_id: PositiveId::new(buyer).unwrap(),
    };
    let target = PositiveId::new(chapter).unwrap();
    let key = IdempotencyKey::parse(&format!("{}8", "C".repeat(42))).unwrap();
    let command = PurchaseCommand::new(
        actor.user_id,
        WalletId::new(buyer).unwrap(),
        ChapterVersionRef {
            chapter_id: target,
            version_id: PositiveId::new(v1).unwrap(),
        },
        MoneyMinor::new(150).unwrap(),
        key,
    )
    .unwrap();
    let checkout = PgCheckout::new(pool.clone());
    let receipt = checkout.checkout_once(&command).await.unwrap();
    assert_eq!(receipt.balance_after.get(), 350);
    assert!(!receipt.replayed);
    assert_eq!(
        checkout.checkout_once(&command).await.unwrap().purchase_id,
        receipt.purchase_id
    );

    let rights = PgDirectEntitlements::new(pool.clone());
    assert_eq!(
        rights.resolve_entitlement(actor, target, None).await.unwrap(),
        EntitlementDecision::Allowed {
            pinned: receipt.pinned
        }
    );

    let progress = PgReadingSync::new(pool.clone());
    let body = progress
        .purchased_body_for_validation(buyer, receipt.purchase_id.get(), chapter, v1)
        .await
        .unwrap()
        .unwrap();
    let place = ReadingPlace {
        block_index: 1,
        char_offset: 3,
    };
    assert!(valid_text_place(place, &reader_blocks(&body)).is_ok());
    assert_eq!(
        progress
            .save_purchased(buyer, receipt.purchase_id.get(), chapter, v1, place, 0)
            .await
            .unwrap()
            .revision,
        1
    );

    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Вторая версия без старого места') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$1 WHERE chapter_id=$2")
        .bind(v2)
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    let saved = PgReadingSync::new(pool.clone())
        .purchased_state(buyer, receipt.purchase_id.get(), chapter, v1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.version_id, v1);
    assert_eq!(saved.place, place);
    assert_ne!(v1, v2);

    // An author may withdraw from sale, but cannot silently erase a paid V1.
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress
        .purchased_allowed(buyer, receipt.purchase_id.get(), chapter, v1)
        .await
        .unwrap());
    assert_eq!(
        rights.resolve_entitlement(actor, target, None).await.unwrap(),
        EntitlementDecision::Allowed {
            pinned: receipt.pinned
        }
    );

    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        rights.resolve_entitlement(actor, target, None).await.unwrap(),
        EntitlementDecision::Denied {
            reason: DenialReason::Locked
        }
    );
    assert_eq!(
        progress
            .purchased_state(buyer, receipt.purchase_id.get(), chapter, v1)
            .await,
        Err(SyncError::NotFound)
    );
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,'s034-integrated-reversal','refund')",
    )
    .bind(receipt.purchase_id.get())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        rights.resolve_entitlement(actor, target, None).await.unwrap(),
        EntitlementDecision::Denied {
            reason: DenialReason::Refunded
        }
    );
    assert_eq!(
        progress
            .purchased_state(buyer, receipt.purchase_id.get(), chapter, v1)
            .await,
        Err(SyncError::NotFound)
    );
    let ledger_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ledger_count, 1, "replay must never create a second debit");
}
