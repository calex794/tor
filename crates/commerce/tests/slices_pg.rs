//! D-02: only test against disposable tor_ci, never a real user wallet.
use sqlx::PgPool;
use tor_commerce::{
    domain::money::{
        CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
    },
    repositories::checkout::PgCheckout,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

async fn fixture(name: &str, balance: i64, price: i64) -> (PgPool, i64, i64, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    assert!(
        url.ends_with("/tor_ci"),
        "refuse any non-tor_ci database for financial tests"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Commerce CI buyer','$argon2id$ci') RETURNING id",
    )
    .bind(format!("d02_{name}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,$2,'RUB')",
    )
    .bind(actor)
    .bind(balance)
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','Проверка покупки','public') RETURNING id",
    )
    .bind(format!("ci:d02:{name}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let (chapter, version) = chapter_with_offer(&pool, book, 1, price).await;
    (pool, actor, book, chapter, version)
}

async fn chapter_with_offer(pool: &PgPool, book: i64, number: i32, price: i64) -> (i64, i64) {
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,'Платная глава','public','paid',$3) RETURNING id",
    )
    .bind(book)
    .bind(number)
    .bind(price)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Закрытый контент версии 1') RETURNING id",
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

fn command(actor: i64, chapter: i64, version: i64, price: i64, letter: char) -> PurchaseCommand {
    let key = IdempotencyKey::parse(&format!("{}8", letter.to_string().repeat(42))).unwrap();
    PurchaseCommand::new(
        PositiveId::new(actor).unwrap(),
        WalletId::new(actor).unwrap(),
        ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        MoneyMinor::new(price).unwrap(),
        key,
    )
    .unwrap()
}

async fn count(pool: &PgPool, table: &str, user: i64) -> i64 {
    // Fixed test-controlled table whitelist, never a request parameter.
    let sql = match table {
        "facts" => "SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1",
        "ledger" => "SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1",
        "commands" => "SELECT count(*) FROM chapter_purchase_commands WHERE user_id=$1",
        "rights" => "SELECT count(*) FROM library_purchased_chapters WHERE user_id=$1",
        _ => panic!("unexpected test table"),
    };
    sqlx::query_scalar(sql)
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn money_unchanged(pool: &PgPool, actor: i64, initial_balance: i64) {
    let actual: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(actor)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(actual, initial_balance, "wallet must roll back");
    for table in ["facts", "ledger", "commands", "rights"] {
        assert_eq!(count(pool, table, actor).await, 0, "unexpected {table}");
    }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_purchase_replay_uses_one_debit_and_exact_pinned_version() {
    let (pool, actor, _, chapter, version) = fixture("replay", 900, 250).await;
    let store = PgCheckout::new(pool.clone());
    let buy = command(actor, chapter, version, 250, 'A');
    let first = store.checkout_once(&buy).await.unwrap();
    assert!(!first.replayed);
    assert_eq!(first.amount.get(), 250);
    assert_eq!(first.balance_after.get(), 650);
    assert_eq!(first.pinned.version_id.get(), version);
    let second = store.checkout_once(&buy).await.unwrap();
    assert!(second.replayed);
    assert_eq!(second.purchase_id, first.purchase_id);
    assert_eq!(second.balance_after, first.balance_after);
    for table in ["facts", "ledger", "commands", "rights"] {
        assert_eq!(count(&pool, table, actor).await, 1, "{table} duplicated");
    }
    let purchased: i64 = sqlx::query_scalar(
        "SELECT active_version_id FROM library_purchased_chapters
         WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(actor)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purchased, version);

    // Publishing V2 cannot silently replace the paid immutable V1.
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Новая версия, не приобретённая') RETURNING id",
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
    let pinned: i64 = sqlx::query_scalar(
        "SELECT active_version_id FROM library_purchased_chapters
         WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(actor)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pinned, version);
    assert_eq!(
        store.checkout_once(&buy).await.unwrap().purchase_id,
        first.purchase_id
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_conflicting_replay_and_second_key_never_charge_twice() {
    let (pool, actor, _, chapter, version) = fixture("conflict", 900, 250).await;
    let store = PgCheckout::new(pool.clone());
    let buy = command(actor, chapter, version, 250, 'A');
    store.checkout_once(&buy).await.unwrap();
    let changed = command(actor, chapter, version, 251, 'A');
    assert_eq!(
        store.checkout_once(&changed).await,
        Err(CheckoutError::DuplicateKeyDifferentRequest)
    );
    let second_key = command(actor, chapter, version, 250, 'B');
    assert_eq!(
        store.checkout_once(&second_key).await,
        Err(CheckoutError::AlreadyOwned)
    );
    for table in ["facts", "ledger", "commands", "rights"] {
        assert_eq!(count(&pool, table, actor).await, 1, "{table} duplicated");
    }
    let current: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, 650);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_insufficient_funds_and_stale_offer_roll_back_everything() {
    let (pool, actor, _, chapter, version) = fixture("insufficient", 50, 100).await;
    let store = PgCheckout::new(pool.clone());
    let buy = command(actor, chapter, version, 100, 'A');
    assert_eq!(
        store.checkout_once(&buy).await,
        Err(CheckoutError::InsufficientBalance)
    );
    money_unchanged(&pool, actor, 50).await;

    sqlx::query("UPDATE chapters SET price_amount=101 WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.checkout_once(&buy).await,
        Err(CheckoutError::StaleOffer)
    );
    money_unchanged(&pool, actor, 50).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_owner_cannot_purchase_their_own_chapter() {
    let (pool, actor, book, chapter, version) = fixture("owner", 400, 100).await;
    sqlx::query("UPDATE books SET owner_user_id=$1 WHERE id=$2")
        .bind(actor)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let store = PgCheckout::new(pool.clone());
    assert_eq!(
        store
            .checkout_once(&command(actor, chapter, version, 100, 'A'))
            .await,
        Err(CheckoutError::Inaccessible)
    );
    money_unchanged(&pool, actor, 400).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_concurrent_identical_keys_return_the_same_purchase() {
    let (pool, actor, _, chapter, version) = fixture("parallel_replay", 300, 100).await;
    let store = PgCheckout::new(pool.clone());
    let buy = command(actor, chapter, version, 100, 'A');
    let (a, b) = tokio::join!(store.checkout_once(&buy), store.checkout_once(&buy));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.purchase_id, b.purchase_id);
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(count(&pool, "facts", actor).await, 1);
    assert_eq!(count(&pool, "ledger", actor).await, 1);
    assert_eq!(count(&pool, "rights", actor).await, 1);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_concurrent_different_keys_cannot_overspend_wallet() {
    let (pool, actor, book, chapter, version) = fixture("parallel_wallet", 100, 80).await;
    let (second_chapter, second_version) = chapter_with_offer(&pool, book, 2, 80).await;
    let store = PgCheckout::new(pool.clone());
    let first = command(actor, chapter, version, 80, 'A');
    let second = command(actor, second_chapter, second_version, 80, 'B');
    let (a, b) = tokio::join!(store.checkout_once(&first), store.checkout_once(&second));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        a == Err(CheckoutError::InsufficientBalance)
            || b == Err(CheckoutError::InsufficientBalance)
    );
    assert_eq!(count(&pool, "ledger", actor).await, 1);
    assert_eq!(count(&pool, "facts", actor).await, 1);
    assert_eq!(count(&pool, "rights", actor).await, 1);
    let after: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, 20);
}
