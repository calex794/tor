//! D/S035: real disposable PostgreSQL 18, never a live wallet.
//! All batch items, the wallet journal, library rights and replay must commit
//! together or not at all, including concurrent requests.
use sqlx::PgPool;
use tor_commerce::{
    domain::{
        batch_checkout::{BatchChapterOffer, BatchCheckoutError, BatchPurchaseCommand},
        money::{CheckoutError, IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::checkout::PgCheckout,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

struct Fixture {
    pool: PgPool,
    buyer: i64,
    book: i64,
    offers: Vec<BatchChapterOffer>,
}

async fn chapter(pool: &PgPool, book: i64, no: i32, price: i64) -> BatchChapterOffer {
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,'Платная глава','public','paid',$3) RETURNING id",
    )
    .bind(book)
    .bind(no)
    .bind(price)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'S035 private text') RETURNING id",
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
    BatchChapterOffer {
        edition: ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        expected_price: MoneyMinor::new(price).unwrap(),
    }
}

async fn book(pool: &PgPool, owner: i64, source: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','S035 пакет','public',$2) RETURNING id",
    )
    .bind(source)
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn fixture(name: &str, initial_balance: i64) -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    assert!(
        url.starts_with("postgresql://tor_app:")
            && url.contains("@127.0.0.1:5432/")
            && url.ends_with("/tor_ci"),
        "batch checkout tests refuse a non-disposable wallet"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let buyer: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S035 buyer','$argon2id$ci') RETURNING id",
    )
    .bind(format!("s035_buyer_{name}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S035 author','$argon2id$ci') RETURNING id",
    )
    .bind(format!("s035_owner_{name}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,$2,'RUB')",
    )
    .bind(buyer)
    .bind(initial_balance)
    .execute(&pool)
    .await
    .unwrap();
    let book = book(&pool, owner, &format!("ci:s035:{name}")).await;
    let mut offers = Vec::new();
    for (number, price) in [(1, 100), (2, 200), (3, 300)] {
        offers.push(chapter(&pool, book, number, price).await);
    }
    Fixture {
        pool,
        buyer,
        book,
        offers,
    }
}

fn command(actor: i64, offers: &[BatchChapterOffer], key: char) -> BatchPurchaseCommand {
    BatchPurchaseCommand::new(
        PositiveId::new(actor).unwrap(),
        WalletId::new(actor).unwrap(),
        offers.to_vec(),
        IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
    )
    .unwrap()
}

async fn count(pool: &PgPool, actor: i64, table: &str) -> i64 {
    let sql = match table {
        "purchase" => "SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1",
        "journal" => "SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1",
        "rights" => "SELECT count(*) FROM library_purchased_chapters WHERE user_id=$1",
        "commands" => "SELECT count(*) FROM chapter_batch_commands WHERE user_id=$1",
        "items" => "SELECT count(*) FROM chapter_batch_items WHERE user_id=$1",
        "shares" => "SELECT count(*) FROM chapter_purchase_share_snapshots s JOIN chapter_purchase_facts p ON p.id=s.purchase_id WHERE p.user_id=$1",
        _ => panic!("unexpected test-only table"),
    };
    sqlx::query_scalar(sql)
        .bind(actor)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn balance(pool: &PgPool, actor: i64) -> i64 {
    sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
        .bind(actor)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn no_partial(f: &Fixture, original: i64) {
    assert_eq!(balance(&f.pool, f.buyer).await, original);
    for table in [
        "purchase", "journal", "rights", "commands", "items", "shares",
    ] {
        assert_eq!(
            count(&f.pool, f.buyer, table).await,
            0,
            "partial batch left table {table}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_batch_exactly_once_and_historic_receipts() {
    let f = fixture("replay", 1_000).await;
    let store = PgCheckout::new(f.pool.clone());
    let quote = command(f.buyer, &f.offers[..2], 'A');
    let paid = store.checkout_batch_once(&quote).await.unwrap();
    assert!(!paid.replayed);
    assert_eq!(paid.total.get(), 300);
    assert_eq!(paid.balance_after.get(), 700);
    assert_eq!(paid.purchases.len(), 2);
    let tracked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_purchase_facts
         WHERE user_id=$1 AND first_grant_tracking_version='c06-first-issue-v1'",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        tracked, 2,
        "every batch purchase must track first paid grant"
    );
    assert_eq!(paid.purchases[0].balance_after.get(), 900);
    assert_eq!(paid.purchases[1].balance_after.get(), 700);
    let again = store.checkout_batch_once(&quote).await.unwrap();
    assert!(again.replayed);
    assert_eq!(again.total, paid.total);
    assert_eq!(again.balance_after, paid.balance_after);
    for (first, second) in paid.purchases.iter().zip(&again.purchases) {
        assert_eq!(first.purchase_id, second.purchase_id);
        assert!(second.replayed);
    }
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
    for table in ["purchase", "journal", "rights", "items", "shares"] {
        assert_eq!(count(&f.pool, f.buyer, table).await, 2, "{table}");
    }
    assert_eq!(count(&f.pool, f.buyer, "commands").await, 1);
    let changed = command(f.buyer, &f.offers[1..3], 'A');
    assert_eq!(
        store.checkout_batch_once(&changed).await,
        Err(BatchCheckoutError::Checkout(
            CheckoutError::DuplicateKeyDifferentRequest
        ))
    );
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_insufficient_wallet_or_stale_offer_rolls_back_every_item() {
    let f = fixture("insufficient", 150).await;
    let store = PgCheckout::new(f.pool.clone());
    assert_eq!(
        store
            .checkout_batch_once(&command(f.buyer, &f.offers[..2], 'B'))
            .await,
        Err(BatchCheckoutError::Checkout(
            CheckoutError::InsufficientBalance
        ))
    );
    no_partial(&f, 150).await;

    let f = fixture("stale", 1_000).await;
    let store = PgCheckout::new(f.pool.clone());
    let mut items = f.offers[..2].to_vec();
    items[1].expected_price = MoneyMinor::new(201).unwrap();
    assert_eq!(
        store
            .checkout_batch_once(&command(f.buyer, &items, 'C'))
            .await,
        Err(BatchCheckoutError::Checkout(CheckoutError::StaleOffer))
    );
    no_partial(&f, 1_000).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_gone_or_other_book_offer_does_not_charge_any_chapter() {
    let f = fixture("inaccessible", 1_000).await;
    let store = PgCheckout::new(f.pool.clone());
    sqlx::query("UPDATE chapters SET admin_hold=TRUE WHERE id=$1")
        .bind(f.offers[1].edition.chapter_id.get())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .checkout_batch_once(&command(f.buyer, &f.offers[..2], 'D'))
            .await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible))
    );
    no_partial(&f, 1_000).await;

    sqlx::query("UPDATE chapters SET admin_hold=FALSE WHERE id=$1")
        .bind(f.offers[1].edition.chapter_id.get())
        .execute(&f.pool)
        .await
        .unwrap();
    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(f.book)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let other = book(&f.pool, owner, "ci:s035:other_book").await;
    let foreign = chapter(&f.pool, other, 1, 100).await;
    assert_eq!(
        store
            .checkout_batch_once(&command(f.buyer, &[f.offers[0], foreign], 'E'))
            .await,
        Err(BatchCheckoutError::MixedBooks)
    );
    no_partial(&f, 1_000).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_concurrent_same_key_debits_once_and_distinct_keys_cannot_double_own() {
    let f = fixture("concurrent", 1_000).await;
    let store = PgCheckout::new(f.pool.clone());
    let quote = command(f.buyer, &f.offers[..2], 'F');
    let (one, two) = tokio::join!(
        store.checkout_batch_once(&quote),
        store.checkout_batch_once(&quote)
    );
    let a = one.unwrap();
    let b = two.unwrap();
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(a.purchases[0].purchase_id, b.purchases[0].purchase_id);
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
    let different = command(f.buyer, &f.offers[..2], 'G');
    assert_eq!(
        store.checkout_batch_once(&different).await,
        Err(BatchCheckoutError::Checkout(CheckoutError::AlreadyOwned))
    );
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
    assert_eq!(count(&f.pool, f.buyer, "purchase").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "journal").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "commands").await, 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_postgres_refuses_orphaned_batch_command_at_commit() {
    let f = fixture("orphan", 1_000).await;
    let result = sqlx::query(
        "INSERT INTO chapter_batch_commands
           (user_id,command_key,request_sha256,item_count)
         VALUES($1,$2,decode(repeat('00',32),'hex'),2)",
    )
    .bind(f.buyer)
    .bind(format!("{}8", "H".repeat(42)))
    .execute(&f.pool)
    .await;
    assert!(result.is_err(), "an unbacked batch must fail at COMMIT");
    no_partial(&f, 1_000).await;
}
