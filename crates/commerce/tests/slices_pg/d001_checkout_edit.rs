//! D-001: competing author and checkout transactions use the same Book
//! fence. Real PostgreSQL lock observations; no timing-only success criterion.
//! B's author lock order is reproduced on legacy fixture data. S3 upload and
//! attachment integrity remain covered by the separate paid-author E2E.
use super::*;
use sqlx::{Postgres, Transaction};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tor_commerce::domain::batch_checkout::{
    BatchChapterOffer, BatchCheckoutError, BatchPurchaseCommand,
};

async fn backend(tx: &mut Transaction<'_, Postgres>) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **tx)
        .await
        .unwrap()
}

// Observe a genuine blocked backend, not just a task that hasn't been polled.
async fn waiter(pool: &PgPool, blocker: i32) -> i32 {
    timeout(Duration::from_secs(8), async {
        loop {
            let waiting: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity
                 WHERE datname=current_database()
                   AND $1=ANY(pg_blocking_pids(pid))
                 ORDER BY pid LIMIT 1",
            )
            .bind(blocker)
            .fetch_optional(pool)
            .await
            .unwrap();
            if let Some(pid) = waiting {
                return pid;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("expected a real PostgreSQL lock wait")
}

async fn begin_author(pool: &PgPool, book: i64) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='5s'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM books WHERE id=$1 FOR UPDATE")
        .bind(book)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

async fn reserve_v2(pool: &PgPool, chapter: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'D001 new edition') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn publish(mut author: Transaction<'static, Postgres>, chapter: i64, v2: i64) {
    sqlx::query("SELECT id FROM chapters WHERE id=$1 FOR UPDATE")
        .bind(chapter)
        .execute(&mut *author)
        .await
        .expect("checkout must not hold a Chapter FK lock while waiting for Book");
    sqlx::query("SELECT version_id FROM chapter_publications WHERE chapter_id=$1 FOR UPDATE")
        .bind(chapter)
        .execute(&mut *author)
        .await
        .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&mut *author)
        .await
        .unwrap();
    author.commit().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_d001_author_wins_purchase_wait_and_stale_quote_never_debits() {
    let (pool, actor, book, chapter, v1) = fixture("d001_writer_first", 900, 250).await;
    let v2 = reserve_v2(&pool, chapter).await;
    let mut author = begin_author(&pool, book).await;
    let author_pid = backend(&mut author).await;
    let store = PgCheckout::new(pool.clone());
    let quote = command(actor, chapter, v1, 250, 'A');
    let buyer = tokio::spawn(async move { store.checkout_once(&quote).await });
    waiter(&pool, author_pid).await;
    // The old code inserted a command before the Book fence, then waited
    // with a Chapter FK lock. This is the exact author lock inversion.
    publish(author, chapter, v2).await;

    let outcome = timeout(Duration::from_secs(10), buyer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome, Err(CheckoutError::StaleOffer));
    money_unchanged(&pool, actor, 900).await;
    let current: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, v2);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_d001_buyer_wins_author_wait_and_keeps_exact_v1() {
    let (pool, actor, book, chapter, v1) = fixture("d001_buyer_first", 900, 250).await;
    let v2 = reserve_v2(&pool, chapter).await;
    // Stop checkout at its wallet lock, AFTER the shared Book/offer fence.
    let mut wallet = pool.begin().await.unwrap();
    let wallet_pid = backend(&mut wallet).await;
    sqlx::query("SELECT user_id FROM reader_wallet_accounts WHERE user_id=$1 FOR NO KEY UPDATE")
        .bind(actor)
        .execute(&mut *wallet)
        .await
        .unwrap();
    let store = PgCheckout::new(pool.clone());
    let quote = command(actor, chapter, v1, 250, 'B');
    let buyer = tokio::spawn(async move { store.checkout_once(&quote).await });
    let buyer_pid = waiter(&pool, wallet_pid).await;
    let author_pool = pool.clone();
    let author = tokio::spawn(async move {
        let tx = begin_author(&author_pool, book).await;
        publish(tx, chapter, v2).await;
    });
    waiter(&pool, buyer_pid).await;
    wallet.commit().await.unwrap();

    let receipt = timeout(Duration::from_secs(10), buyer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(10), author)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.pinned.version_id.get(), v1);
    assert_eq!(receipt.balance_after.get(), 650);
    let pinned_and_current: (i64, i64) = sqlx::query_as(
        "SELECT l.active_version_id,p.version_id
         FROM library_purchased_chapters l
         JOIN chapter_publications p ON p.chapter_id=l.chapter_id
         WHERE l.user_id=$1 AND l.chapter_id=$2",
    )
    .bind(actor)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pinned_and_current, (v1, v2));
    let replay = PgCheckout::new(pool.clone())
        .checkout_once(&command(actor, chapter, v1, 250, 'B'))
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.purchase_id, receipt.purchase_id);
    for table in ["facts", "ledger", "commands", "rights"] {
        assert_eq!(count(&pool, table, actor).await, 1);
    }
    let logical: i64 = sqlx::query_scalar(
        "SELECT reader_retained_editions FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(logical, 2);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_d001_batch_waits_for_author_and_rolls_back_entire_stale_quote() {
    let (pool, actor, book, chapter, v1) = fixture("d001_batch_writer_first", 900, 250).await;
    let (other, other_version) = chapter_with_offer(&pool, book, 2, 150).await;
    let v2 = reserve_v2(&pool, chapter).await;
    let mut author = begin_author(&pool, book).await;
    let author_pid = backend(&mut author).await;
    let quote = BatchPurchaseCommand::new(
        PositiveId::new(actor).unwrap(),
        WalletId::new(actor).unwrap(),
        vec![
            BatchChapterOffer {
                edition: command(actor, chapter, v1, 250, 'C').edition,
                expected_price: MoneyMinor::new(250).unwrap(),
            },
            BatchChapterOffer {
                edition: command(actor, other, other_version, 150, 'D').edition,
                expected_price: MoneyMinor::new(150).unwrap(),
            },
        ],
        IdempotencyKey::parse(&format!("{}8", "E".repeat(42))).unwrap(),
    )
    .unwrap();
    let store = PgCheckout::new(pool.clone());
    let buyer = tokio::spawn(async move { store.checkout_batch_once(&quote).await });
    waiter(&pool, author_pid).await;
    publish(author, chapter, v2).await;
    let outcome = timeout(Duration::from_secs(10), buyer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome,
        Err(BatchCheckoutError::Checkout(CheckoutError::StaleOffer))
    );
    money_unchanged(&pool, actor, 900).await;
    let commands: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_batch_commands WHERE user_id=$1")
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(commands, 0);
}
