//! D/S035: real disposable PostgreSQL 18, never a live wallet.
//! All batch items, the wallet journal, library rights and replay must commit
//! together or not at all, including concurrent requests.
use sqlx::PgPool;
use tor_commerce::{
    domain::{
        batch_checkout::{BatchChapterOffer, BatchCheckoutError, BatchPurchaseCommand},
        money::{CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, WalletId},
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
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','S035 пакет','public',$2,'basic') RETURNING id",
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn h_ui_19_batch_checkout_does_not_bypass_package_paid_exclusivity() {
    use tor_commerce::repositories::{layer_package_sales::PackagePaidMode, layers::PgLayers};
    let f = fixture("h19-batch-exclusive", 1_000).await;
    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(f.book)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let layers = PgLayers::new(f.pool.clone());
    let layer = layers
        .create_layer(PositiveId::new(owner).unwrap(), "Book package only")
        .await
        .unwrap();
    let version = layers
        .add_book(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(f.book).unwrap(),
        )
        .await
        .unwrap();
    layers
        .publish_offer(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            250,
            true,
        )
        .await
        .unwrap();
    let store = PgCheckout::new(f.pool.clone());
    assert_eq!(
        store
            .selectable_batch_offers(f.buyer, f.book)
            .await
            .unwrap()
            .len(),
        3
    );
    let revision = layers
        .set_package_paid_mode(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(f.book).unwrap(),
            PositiveId::new(version).unwrap(),
            0,
            PackagePaidMode::PackageOnly,
        )
        .await
        .unwrap();
    assert!(revision > 0);
    assert!(store
        .selectable_batch_offers(f.buyer, f.book)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .checkout_batch_once(&command(f.buyer, &f.offers[..2], 'J'))
            .await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible)),
        "forged native batch checkout cannot debit a package-only Book",
    );
    no_partial(&f, 1_000).await;
    layers
        .set_package_paid_mode(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(f.book).unwrap(),
            PositiveId::new(version).unwrap(),
            revision,
            PackagePaidMode::Direct,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .selectable_batch_offers(f.buyer, f.book)
            .await
            .unwrap()
            .len(),
        3
    );
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn h_ui_21_batch_mixed_registration_cannot_partially_debit() {
    use tor_commerce::repositories::{layer_package_sales::PackagePaidMode, layers::PgLayers};
    let f = fixture("h21-mixed", 1_000).await;
    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(f.book)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let layers = PgLayers::new(f.pool.clone());
    let layer = layers
        .create_layer(PositiveId::new(owner).unwrap(), "Windowed")
        .await
        .unwrap();
    let version = layers
        .add_book(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(f.book).unwrap(),
        )
        .await
        .unwrap();
    layers
        .publish_offer(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            250,
            true,
        )
        .await
        .unwrap();
    let early: i64 = sqlx::query_scalar(
        "INSERT INTO chapters
           (book_id,chapter_no,title,visibility,access_level,price_amount,registered_at)
         VALUES ($1,4,'Old registered paid','public','paid',100,
                 now()-interval '20 days') RETURNING id",
    )
    .bind(f.book)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let edition: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'S035 private text') RETURNING id",
    )
    .bind(early)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(early)
        .bind(edition)
        .execute(&f.pool)
        .await
        .unwrap();
    let prior = BatchChapterOffer {
        edition: ChapterVersionRef {
            chapter_id: PositiveId::new(early).unwrap(),
            version_id: PositiveId::new(edition).unwrap(),
        },
        expected_price: MoneyMinor::new(100).unwrap(),
    };
    layers
        .set_package_paid_mode(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(f.book).unwrap(),
            PositiveId::new(version).unwrap(),
            0,
            PackagePaidMode::EarlyDays(10),
        )
        .await
        .unwrap();
    let checkout = PgCheckout::new(f.pool.clone());
    let available = checkout
        .selectable_batch_offers(f.buyer, f.book)
        .await
        .unwrap();
    assert_eq!(available.len(), 1);
    assert_eq!(available[0].chapter_id, early);
    assert_eq!(
        checkout
            .checkout_batch_once(&command(f.buyer, &[prior, f.offers[0]], 'Q'))
            .await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible)),
        "one newly registered locked chapter aborts the whole mixed batch",
    );
    no_partial(&f, 1_000).await;
    let first = checkout
        .checkout_once(
            &tor_commerce::domain::money::PurchaseCommand::new(
                PositiveId::new(f.buyer).unwrap(),
                WalletId::new(f.buyer).unwrap(),
                prior.edition,
                MoneyMinor::new(100).unwrap(),
                IdempotencyKey::parse(&format!("{}8", "R".repeat(42))).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(!first.replayed);
    assert_eq!(balance(&f.pool, f.buyer).await, 900);
    f.pool.close().await;
}
/// P0 S035/D-06/C-06: a batch creates two paid rights, not two first opens.
/// A concurrent revocation must remain authoritative even if a first-grant
/// SELECT began before the revocation committed and waited for a row lock.
/// This is a database fence test, not proof of S3 delivery or administrator
/// support refund approval.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_first_grant_per_chapter_and_revocation_wins_lock_wait() {
    use std::time::Duration;
    use tor_commerce::repositories::direct_exposure::{DirectGrantKind, PgDirectFirstExposure};
    use tor_contracts::paid_delivery::DeliveryError;

    let f = fixture("first-grant-fence", 1_000).await;
    let quote = command(f.buyer, &f.offers[..2], 'K');
    let bought = PgCheckout::new(f.pool.clone())
        .checkout_batch_once(&quote)
        .await
        .unwrap();
    let first_purchase = bought.purchases[0].purchase_id;
    let second_purchase = bought.purchases[1].purchase_id;
    let first = f.offers[0].edition;
    let second = f.offers[1].edition;
    let exposure = PgDirectFirstExposure::new(f.pool.clone());

    let before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(before, 0, "checkout is not a first grant");
    let first_readable: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(f.buyer)
            .bind(first_purchase.get())
            .bind(first.chapter_id.get())
            .bind(first.version_id.get())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(
        first_readable,
        "the paid grant fixture must pass live S020 ACL"
    );

    exposure
        .record(
            PositiveId::new(f.buyer).unwrap(),
            first.chapter_id,
            first.version_id,
            Some(PositiveId::new(first_purchase.get()).unwrap()),
            DirectGrantKind::Chapter,
        )
        .await
        .unwrap();
    // Reopening the same purchased V1, even through an asset grant, must
    // not create a second evidence row or overwrite the first event kind.
    exposure
        .record(
            PositiveId::new(f.buyer).unwrap(),
            first.chapter_id,
            first.version_id,
            Some(PositiveId::new(first_purchase.get()).unwrap()),
            DirectGrantKind::Asset,
        )
        .await
        .unwrap();
    let after_first: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(after_first, 1);
    let first_kind: String = sqlx::query_scalar(
        "SELECT object_kind FROM chapter_purchase_first_exposures WHERE purchase_id=$1",
    )
    .bind(first_purchase.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(first_kind, "chapter");
    let second_exposures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE purchase_id=$1",
    )
    .bind(second_purchase.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(second_exposures, 0, "chapter B must remain unopened");

    // Lock purchase B before the first-issue recorder begins. Revocation is
    // committed while holding this lock; the blocked recorder must recheck
    // the canonical paid entitlement in a fresh READ COMMITTED statement.
    let mut reversing = f.pool.begin().await.unwrap();
    let locked: i64 =
        sqlx::query_scalar("SELECT id FROM chapter_purchase_facts WHERE id=$1 FOR UPDATE")
            .bind(second_purchase.get())
            .fetch_one(&mut *reversing)
            .await
            .unwrap();
    assert_eq!(locked, second_purchase.get());

    let buyer = f.buyer;
    let pool = f.pool.clone();
    let mut blocked_grant = tokio::spawn(async move {
        PgDirectFirstExposure::new(pool)
            .record(
                PositiveId::new(buyer).unwrap(),
                second.chapter_id,
                second.version_id,
                Some(PositiveId::new(second_purchase.get()).unwrap()),
                DirectGrantKind::Chapter,
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut blocked_grant)
            .await
            .is_err(),
        "the first-grant recorder must wait for the purchase row lock"
    );

    sqlx::query(
        "INSERT INTO chapter_purchase_revocations
           (purchase_id,reversal_reference,reason) VALUES($1,$2,'refund')",
    )
    .bind(second_purchase.get())
    .bind(format!("ci:s035:grant-fence:{}", second_purchase.get()))
    .execute(&mut *reversing)
    .await
    .unwrap();
    reversing.commit().await.unwrap();

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), blocked_grant)
            .await
            .expect("grant/revocation lock must not deadlock")
            .expect("join blocked first-grant task"),
        Err(DeliveryError::Denied),
    );
    let after_revoke: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(after_revoke, 1);
    let second_opened: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM chapter_purchase_first_exposures WHERE purchase_id=$1)",
    )
    .bind(second_purchase.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(
        !second_opened,
        "a revoked batch item cannot gain first-use evidence"
    );
    let second_readable: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(f.buyer)
            .bind(second_purchase.get())
            .bind(second.chapter_id.get())
            .bind(second.version_id.get())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(!second_readable);
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
    assert_eq!(count(&f.pool, f.buyer, "purchase").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "rights").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "journal").await, 2);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_unknown_book_age_tier_cannot_charge_and_recovers_after_attestation() {
    let f = fixture("unknown-book-age", 1_000).await;
    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();

    let store = PgCheckout::new(f.pool.clone());
    assert!(
        store
            .selectable_batch_offers(f.buyer, f.book)
            .await
            .unwrap()
            .is_empty(),
        "unknown age cannot be offered for purchase"
    );
    let quote = command(f.buyer, &f.offers[..2], 'N');
    assert_eq!(
        store.checkout_batch_once(&quote).await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible)),
    );
    no_partial(&f, 1_000).await;

    // An authoritative B update restores eligibility. A rejected command
    // cannot reserve the key or create an unrefundable debit.
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .selectable_batch_offers(f.buyer, f.book)
            .await
            .unwrap()
            .len(),
        3
    );
    let receipt = store.checkout_batch_once(&quote).await.unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.purchases.len(), 2);
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
    assert_eq!(count(&f.pool, f.buyer, "purchase").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "rights").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "journal").await, 2);
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_age_filtered_batch_offers_and_no_debit_for_ineligible_buyer() {
    let f = fixture("batch-adult-eligibility", 1_000).await;
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();
    let store = PgCheckout::new(f.pool.clone());
    let quote = command(f.buyer, &f.offers[..2], 'R');

    assert!(store
        .selectable_batch_offers(f.buyer, f.book)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.checkout_batch_once(&quote).await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible))
    );
    no_partial(&f, 1_000).await;

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(f.buyer)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .selectable_batch_offers(f.buyer, f.book)
            .await
            .unwrap()
            .len(),
        3
    );
    let bought = store.checkout_batch_once(&quote).await.unwrap();
    assert!(!bought.replayed);
    assert_eq!(bought.purchases.len(), 2);

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(f.buyer)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(store
        .selectable_batch_offers(f.buyer, f.book)
        .await
        .unwrap()
        .is_empty());
    assert!(store.checkout_batch_once(&quote).await.unwrap().replayed);
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
    assert_eq!(count(&f.pool, f.buyer, "purchase").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "rights").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "journal").await, 2);
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_pro_grant_and_suspended_buyer_cannot_buy_without_current_authority() {
    let f = fixture("batch-pro-eligibility", 1_000).await;
    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();
    let store = PgCheckout::new(f.pool.clone());
    let quote = command(f.buyer, &f.offers[..2], 'S');
    assert!(store
        .selectable_batch_offers(f.buyer, f.book)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.checkout_batch_once(&quote).await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible))
    );
    no_partial(&f, 1_000).await;

    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(f.book)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(owner)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',FALSE)",
    )
    .bind(f.buyer)
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(store
        .selectable_batch_offers(f.buyer, f.book)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.checkout_batch_once(&quote).await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible))
    );
    no_partial(&f, 1_000).await;

    sqlx::query("UPDATE user_level_grants SET is_active=TRUE WHERE user_id=$1")
        .bind(f.buyer)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .selectable_batch_offers(f.buyer, f.book)
            .await
            .unwrap()
            .len(),
        3
    );

    // Even a verified pro grant cannot bypass account suspension.
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(f.buyer)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(store
        .selectable_batch_offers(f.buyer, f.book)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.checkout_batch_once(&quote).await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible))
    );
    no_partial(&f, 1_000).await;

    sqlx::query("UPDATE users SET account_state='active' WHERE id=$1")
        .bind(f.buyer)
        .execute(&f.pool)
        .await
        .unwrap();
    let bought = store.checkout_batch_once(&quote).await.unwrap();
    assert_eq!(bought.purchases.len(), 2);
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
    f.pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_pro_grant_revocation_wins_waiting_batch_checkout() {
    use std::time::Duration;
    let f = fixture("batch-pro-revoke-race", 1_000).await;
    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(f.buyer)
    .execute(&f.pool)
    .await
    .unwrap();
    let mut revoking = f.pool.begin().await.unwrap();
    let locked: i64 = sqlx::query_scalar(
        "SELECT user_id FROM user_level_grants
         WHERE user_id=$1 AND level='pro_adult' AND is_active FOR UPDATE",
    )
    .bind(f.buyer)
    .fetch_one(&mut *revoking)
    .await
    .unwrap();
    assert_eq!(locked, f.buyer);

    let quote = command(f.buyer, &f.offers[..2], 'T');
    let checkout = PgCheckout::new(f.pool.clone());
    let mut task = tokio::spawn(async move { checkout.checkout_batch_once(&quote).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut task)
            .await
            .is_err(),
        "batch checkout must wait for the pro grant row lock"
    );
    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(f.buyer)
    .execute(&mut *revoking)
    .await
    .unwrap();
    revoking.commit().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .expect("batch age lock must release")
            .expect("batch task must join"),
        Err(BatchCheckoutError::Checkout(CheckoutError::Inaccessible))
    );
    no_partial(&f, 1_000).await;
    f.pool.close().await;
}
