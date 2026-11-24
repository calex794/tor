//! D-001 / D-02 / S035: exact-edition checkout against an author publication.
//! The editor fixture holds the same Book -> Chapter -> publication row locks
//! as B's publication transaction. No real S3, network, money or GC is used.
use super::*;
use sqlx::{Postgres, Transaction};
use std::time::Duration;
use tor_commerce::domain::money::{PurchaseCommand, PurchaseReceipt};

#[derive(Clone, Copy)]
enum Mode {
    Direct,
    Batch,
}

impl Mode {
    fn offers(self, f: &Fixture) -> Vec<BatchChapterOffer> {
        match self {
            Self::Direct => f.offers[..1].to_vec(),
            Self::Batch => f.offers[..2].to_vec(),
        }
    }
}

async fn buy(
    store: PgCheckout,
    actor: i64,
    mode: Mode,
    offers: Vec<BatchChapterOffer>,
    key: char,
) -> Result<Vec<PurchaseReceipt>, BatchCheckoutError> {
    match mode {
        Mode::Direct => {
            let item = &offers[0];
            let command = PurchaseCommand::new(
                PositiveId::new(actor).unwrap(),
                WalletId::new(actor).unwrap(),
                item.edition,
                item.expected_price,
                IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
            )
            .unwrap();
            store
                .checkout_once(&command)
                .await
                .map(|receipt| vec![receipt])
                .map_err(BatchCheckoutError::Checkout)
        }
        Mode::Batch => store
            .checkout_batch_once(&command(actor, &offers, key))
            .await
            .map(|receipt| receipt.purchases),
    }
}

async fn prepare_version(pool: &PgPool, chapter: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'D001 replacement V2') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn backend_pid(tx: &mut Transaction<'_, Postgres>) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **tx)
        .await
        .unwrap()
}

// Observe the actual blocker PID, rather than treating an arbitrary sleep
// or a slow runner as evidence that the expected SQL lock was acquired.
async fn wait_blocked_by(pool: &PgPool, blocker: i32) -> i32 {
    for _ in 0..160 {
        let blocked: Option<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity
             WHERE pid<>$1 AND $1=ANY(pg_blocking_pids(pid))
             ORDER BY pid LIMIT 1",
        )
        .bind(blocker)
        .fetch_optional(pool)
        .await
        .unwrap();
        if let Some(pid) = blocked {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("D-001 transaction never waited for the expected lock");
}

async fn switch_publication(
    tx: &mut Transaction<'_, Postgres>,
    book: i64,
    chapter: i64,
    v1: i64,
    v2: i64,
) {
    let locked_book: i64 = sqlx::query_scalar("SELECT id FROM books WHERE id=$1 FOR UPDATE")
        .bind(book)
        .fetch_one(&mut **tx)
        .await
        .unwrap();
    assert_eq!(locked_book, book);
    let locked_chapter: i64 =
        sqlx::query_scalar("SELECT id FROM chapters WHERE id=$1 AND book_id=$2 FOR UPDATE")
            .bind(chapter)
            .bind(book)
            .fetch_one(&mut **tx)
            .await
            .unwrap();
    assert_eq!(locked_chapter, chapter);
    let changed = sqlx::query(
        "UPDATE chapter_publications SET version_id=$3
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v1)
    .bind(v2)
    .execute(&mut **tx)
    .await
    .unwrap();
    assert_eq!(changed.rows_affected(), 1);
}

async fn assert_pins(
    f: &Fixture,
    offers: &[BatchChapterOffer],
    receipts: &[PurchaseReceipt],
) {
    assert_eq!(receipts.len(), offers.len());
    for (offer, receipt) in offers.iter().zip(receipts) {
        assert_eq!(receipt.pinned, offer.edition);
        assert_eq!(receipt.amount, offer.expected_price);
        let stored: (i64, i64) = sqlx::query_as(
            "SELECT p.version_id,l.active_version_id
             FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l
               ON l.purchase_id=p.id AND l.user_id=p.user_id
             WHERE p.user_id=$1 AND p.chapter_id=$2",
        )
        .bind(f.buyer)
        .bind(offer.edition.chapter_id.get())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let version = offer.edition.version_id.get();
        assert_eq!(stored, (version, version));
    }
    let total: i64 = offers.iter().map(|item| item.expected_price.get()).sum();
    assert_eq!(balance(&f.pool, f.buyer).await, 1_000 - total);
    for table in ["purchase", "journal", "rights", "shares"] {
        assert_eq!(
            count(&f.pool, f.buyer, table).await,
            offers.len() as i64,
            "unexpected count for {table}"
        );
    }
}

async fn editor_wins(mode: Mode, name: &str) {
    let f = fixture(name, 1_000).await;
    let mut offers = mode.offers(&f);
    // In batch mode change the second item: no first-item debit may escape.
    let changed_index = offers.len() - 1;
    let old = offers[changed_index].edition;
    let v2 = prepare_version(&f.pool, old.chapter_id.get()).await;

    let mut editor = f.pool.begin().await.unwrap();
    let editor_pid = backend_pid(&mut editor).await;
    switch_publication(
        &mut editor,
        f.book,
        old.chapter_id.get(),
        old.version_id.get(),
        v2,
    )
    .await;

    let store = PgCheckout::new(f.pool.clone());
    let task = tokio::spawn(buy(store.clone(), f.buyer, mode, offers.clone(), 'R'));
    wait_blocked_by(&f.pool, editor_pid).await;
    editor.commit().await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("publication must release checkout")
        .expect("checkout task must join");
    assert_eq!(result, Err(BatchCheckoutError::Checkout(CheckoutError::StaleOffer)));
    no_partial(&f, 1_000).await;
    let direct_claims: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_purchase_commands WHERE user_id=$1")
            .bind(f.buyer)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(direct_claims, 0, "stale direct command must roll back");

    // A newly quoted exact V2 succeeds; stale quote did not consume its key.
    offers[changed_index].edition.version_id = PositiveId::new(v2).unwrap();
    let purchased = buy(store, f.buyer, mode, offers.clone(), 'R')
        .await
        .unwrap();
    assert_pins(&f, &offers, &purchased).await;
    f.pool.close().await;
}

async fn buyer_wins(mode: Mode, name: &str) {
    let f = fixture(name, 1_000).await;
    let offers = mode.offers(&f);
    let old = offers[offers.len() - 1].edition;
    let v2 = prepare_version(&f.pool, old.chapter_id.get()).await;

    // Force checkout to wait at its wallet lock AFTER acquiring offer locks.
    let mut wallet = f.pool.begin().await.unwrap();
    let wallet_pid = backend_pid(&mut wallet).await;
    let locked_balance: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1 FOR UPDATE",
    )
    .bind(f.buyer)
    .fetch_one(&mut *wallet)
    .await
    .unwrap();
    assert_eq!(locked_balance, 1_000);

    let store = PgCheckout::new(f.pool.clone());
    let buyer = tokio::spawn(buy(store.clone(), f.buyer, mode, offers.clone(), 'S'));
    let buyer_pid = wait_blocked_by(&f.pool, wallet_pid).await;

    let pool = f.pool.clone();
    let book = f.book;
    let editor = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        switch_publication(
            &mut tx,
            book,
            old.chapter_id.get(),
            old.version_id.get(),
            v2,
        )
        .await;
        tx.commit().await.unwrap();
    });
    // Publication must wait for the buyer's exact-version/Book fence.
    wait_blocked_by(&f.pool, buyer_pid).await;
    wallet.commit().await.unwrap();

    let purchased = tokio::time::timeout(Duration::from_secs(15), buyer)
        .await
        .expect("wallet release must unblock buyer")
        .expect("buyer task must join")
        .expect("V1 purchase must succeed before author switch");
    tokio::time::timeout(Duration::from_secs(15), editor)
        .await
        .expect("buyer commit must unblock editor")
        .expect("editor task must join");

    let current: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(old.chapter_id.get())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(current, v2);
    assert_pins(&f, &offers, &purchased).await;

    // Retrying the exact original command after publication must return V1.
    let replay = buy(store, f.buyer, mode, offers.clone(), 'S')
        .await
        .unwrap();
    assert_pins(&f, &offers, &replay).await;
    for (original, repeated) in purchased.iter().zip(&replay) {
        assert!(repeated.replayed);
        assert_eq!(original.purchase_id, repeated.purchase_id);
        assert_eq!(original.pinned, repeated.pinned);
        assert_eq!(original.balance_after, repeated.balance_after);
    }
    let retained: (i64, i64, i64) = sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions,physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(old.chapter_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(retained, (2, 2, 2));
    f.pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_d001_direct_checkout_publication_lock_order() {
    editor_wins(Mode::Direct, "d001-direct-editor-first").await;
    buyer_wins(Mode::Direct, "d001-direct-buyer-first").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_d001_batch_checkout_publication_lock_order() {
    editor_wins(Mode::Batch, "d001-batch-editor-first").await;
    buyer_wins(Mode::Batch, "d001-batch-buyer-first").await;
}
