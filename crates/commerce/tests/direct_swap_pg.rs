//! D-06: unpaired money/rights movements must never commit before swap exists.
//! Test only on a disposable PostgreSQL 18 tor_ci; no operator finance policy.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use tor_commerce::{
    domain::money::{
        CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
    },
    repositories::checkout::PgCheckout,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

struct Case {
    pool: PgPool,
    buyer: i64,
    source_purchase: i64,
    target_chapter: i64,
    target_version: i64,
}

async fn paid_chapter(pool: &PgPool, book: i64, number: i32, price: i64) -> (i64, i64) {
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,'Direct swap test chapter','public','paid',$3) RETURNING id",
    )
    .bind(book)
    .bind(number)
    .bind(price)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Private test chapter') RETURNING id",
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

async fn fixture() -> Case {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable database URL required");
    assert!(url.ends_with("/tor_ci"), "refuse a non-disposable database");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let buyer: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Swap test buyer','$argon2id$ci') RETURNING id",
    )
    .bind(format!("swap-buyer-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Swap test owner','$argon2id$ci') RETURNING id",
    )
    .bind(format!("swap-owner-{nonce}@example.invalid"))
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

    let mut books = Vec::new();
    for idx in [1, 2] {
        let book: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES($1,'ru','D-06 swap fixture','public',$2) RETURNING id",
        )
        .bind(format!("ci:d06:swap:{nonce}:{idx}"))
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
        books.push(book);
    }
    let (source_chapter, source_version) = paid_chapter(&pool, books[0], 1, 100).await;
    let (target_chapter, target_version) = paid_chapter(&pool, books[1], 1, 150).await;
    let purchased = PgCheckout::new(pool.clone())
        .checkout_once(
            &PurchaseCommand::new(
                PositiveId::new(buyer).unwrap(),
                WalletId::new(buyer).unwrap(),
                ChapterVersionRef {
                    chapter_id: PositiveId::new(source_chapter).unwrap(),
                    version_id: PositiveId::new(source_version).unwrap(),
                },
                MoneyMinor::new(100).unwrap(),
                IdempotencyKey::parse(&format!("{}8", "S".repeat(42))).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(purchased.balance_after.get(), 400);
    Case {
        pool,
        buyer,
        source_purchase: purchased.purchase_id.get(),
        target_chapter,
        target_version,
    }
}

fn claim() -> &'static str {
    "INSERT INTO finance_direct_swap_commands
       (buyer_user_id,request_key,request_digest,source_purchase_id,
        target_chapter_id,target_version_id,source_amount_minor,target_amount_minor)
     VALUES ($1,$2,$3,$4,$5,$6,100,150)"
}

async fn assert_original_unchanged(f: &Case) {
    let balance: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(balance, 400, "a rejected swap cannot modify the buyer wallet");
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_direct_swap_commands WHERE buyer_user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(claims, 0, "an incomplete idempotency claim must not survive");
    let postings: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_direct_swap_postings WHERE buyer_user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(postings, 0);
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM library_purchased_chapters WHERE user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(active, 1, "old access survives a failed private swap");
}

fn is_integrity_rejection(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|db| db.code())
        .as_deref()
        == Some("23514")
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with PR #374 migration"]
async fn d_06_swap_claim_without_balanced_postings_cannot_commit() {
    let f = fixture().await;
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query(claim())
        .bind(f.buyer)
        .bind(format!("{}8", "A".repeat(42)))
        .bind(vec![3_u8; 32])
        .bind(f.source_purchase)
        .bind(f.target_chapter)
        .bind(f.target_version)
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = tx.commit().await.expect_err("unfinished claim must roll back");
    assert!(is_integrity_rejection(&err), "unexpected SQL error: {err}");
    assert_original_unchanged(&f).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with PR #374 migration"]
async fn d_06_swap_intent_is_immutable_before_any_settlement() {
    let f = fixture().await;
    let mut tx = f.pool.begin().await.unwrap();
    let key = format!("{}8", "B".repeat(42));
    sqlx::query(claim())
        .bind(f.buyer)
        .bind(&key)
        .bind(vec![4_u8; 32])
        .bind(f.source_purchase)
        .bind(f.target_chapter)
        .bind(f.target_version)
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = sqlx::query(
        "UPDATE finance_direct_swap_commands SET target_amount_minor=151
         WHERE buyer_user_id=$1 AND request_key=$2",
    )
    .bind(f.buyer)
    .bind(&key)
    .execute(&mut *tx)
    .await
    .expect_err("idempotency identity cannot be changed");
    assert!(is_integrity_rejection(&err), "unexpected SQL error: {err}");
    tx.rollback().await.unwrap();
    assert_original_unchanged(&f).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with PR #374 migration"]
async fn d_06_swap_wallet_credit_without_finance_posting_cannot_commit() {
    let f = fixture().await;
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE reader_wallet_accounts SET balance_minor=500 WHERE user_id=$1",
    )
    .bind(f.buyer)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_entries
         (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
         VALUES($1,'chapter_swap_credit',100,500,$2,$3)",
    )
    .bind(f.buyer)
    .bind(format!("d06:orphan-swap:{}", f.source_purchase))
    .bind(f.source_purchase)
    .execute(&mut *tx)
    .await
    .unwrap();
    let err = tx.commit().await.expect_err("orphan wallet credit must fail");
    assert!(is_integrity_rejection(&err), "unexpected SQL error: {err}");
    assert_original_unchanged(&f).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with PR #374 migration"]
async fn d_06_swap_revocation_without_finance_posting_cannot_commit() {
    let f = fixture().await;
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations
         (purchase_id,reversal_reference,reason)
         VALUES($1,$2,'purchase_swap')",
    )
    .bind(f.source_purchase)
    .bind(format!("d06:orphan-revocation:{}", f.source_purchase))
    .execute(&mut *tx)
    .await
    .unwrap();
    let err = tx.commit().await.expect_err("orphan swap revocation must fail");
    assert!(is_integrity_rejection(&err), "unexpected SQL error: {err}");
    assert_original_unchanged(&f).await;
}
