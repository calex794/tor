//! D-02: only test against disposable tor_ci, never a real user wallet.
use sqlx::PgPool;
use tor_commerce::repositories::entitlements::PgDirectEntitlements;
use tor_commerce::{
    domain::money::{
        CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
    },
    repositories::checkout::PgCheckout,
};
use tor_contracts::paid_delivery::{
    Actor, ChapterVersionRef, DenialReason, EntitlementDecision, EntitlementPort, PositiveId,
};

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
    // Every sellable owned Book has a server-derived B-10 economic schedule.
    // Anonymous legacy catalog rows must never fabricate historical shares.
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Commerce CI owner','$argon2id$ci') RETURNING id",
    )
    .bind(format!("d02_owner_{name}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Проверка покупки','public',$2) RETURNING id",
    )
    .bind(format!("ci:d02:{name}"))
    .bind(owner)
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

fn paid_actor(user_id: i64) -> Actor {
    Actor {
        user_id: PositiveId::new(user_id).unwrap(),
    }
}

fn denied(reason: DenialReason) -> EntitlementDecision {
    EntitlementDecision::Denied { reason }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_03_unpurchased_even_with_saved_progress_cannot_mint_rights() {
    let (pool, actor, book, chapter, version) = fixture("d03_progress", 500, 100).await;
    sqlx::query(
        "INSERT INTO reader_book_positions
         (user_id,book_id,chapter_id,chapter_no,chapter_revision,version_id,block_index)
         VALUES ($1,$2,$3,1,0,$4,1)",
    )
    .bind(actor)
    .bind(book)
    .bind(chapter)
    .bind(version)
    .execute(&pool)
    .await
    .unwrap();
    let repo = PgDirectEntitlements::new(pool);
    let decision = repo
        .resolve_entitlement(paid_actor(actor), PositiveId::new(chapter).unwrap(), None)
        .await
        .unwrap();
    assert_eq!(decision, denied(DenialReason::NotPurchased));
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_03_only_buyer_can_read_exact_v1_after_new_publication_v2() {
    let (pool, actor, _, chapter, version) = fixture("d03_pin", 500, 100).await;
    let checkout = PgCheckout::new(pool.clone());
    let receipt = checkout
        .checkout_once(&command(actor, chapter, version, 100, 'A'))
        .await
        .unwrap();
    let rights = PgDirectEntitlements::new(pool.clone());
    let chapter_id = PositiveId::new(chapter).unwrap();
    let purchased = rights
        .resolve_entitlement(
            paid_actor(actor),
            chapter_id,
            Some(PositiveId::new(receipt.purchase_id.get()).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(
        purchased,
        EntitlementDecision::Allowed {
            pinned: receipt.pinned
        }
    );

    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('d03_stranger@example.test','Another buyer','$argon2id$ci') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        rights
            .resolve_entitlement(
                paid_actor(stranger),
                chapter_id,
                Some(PositiveId::new(receipt.purchase_id.get()).unwrap())
            )
            .await
            .unwrap(),
        denied(DenialReason::NotPurchased)
    );
    assert_eq!(
        rights
            .resolve_entitlement(
                paid_actor(actor),
                chapter_id,
                Some(PositiveId::new(receipt.purchase_id.get() + 1).unwrap())
            )
            .await
            .unwrap(),
        denied(DenialReason::NotPurchased)
    );

    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Новая редакция') RETURNING id",
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
    assert_eq!(
        rights
            .resolve_entitlement(paid_actor(actor), chapter_id, None)
            .await
            .unwrap(),
        purchased
    );
    assert_ne!(version, v2);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_03_refund_immediately_denies_future_entitlement_decisions() {
    let (pool, actor, _, chapter, version) = fixture("d03_refund", 500, 100).await;
    let receipt = PgCheckout::new(pool.clone())
        .checkout_once(&command(actor, chapter, version, 100, 'A'))
        .await
        .unwrap();
    let rights = PgDirectEntitlements::new(pool.clone());
    let chapter_id = PositiveId::new(chapter).unwrap();
    assert!(matches!(
        rights
            .resolve_entitlement(paid_actor(actor), chapter_id, None)
            .await
            .unwrap(),
        EntitlementDecision::Allowed { .. }
    ));
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,'d03_refund_reversal','refund')",
    )
    .bind(receipt.purchase_id.get())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        rights
            .resolve_entitlement(paid_actor(actor), chapter_id, None)
            .await
            .unwrap(),
        denied(DenialReason::Refunded)
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_03_visibility_does_not_erase_purchase_but_hard_holds_always_deny() {
    let (pool, actor, book, chapter, version) = fixture("d03_hold", 500, 100).await;
    PgCheckout::new(pool.clone())
        .checkout_once(&command(actor, chapter, version, 100, 'A'))
        .await
        .unwrap();
    let rights = PgDirectEntitlements::new(pool.clone());
    let chapter_id = PositiveId::new(chapter).unwrap();
    let buyer = paid_actor(actor);
    // Creator hiding the storefront cannot revoke a valid paid V1.
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET visibility='draft' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        rights
            .resolve_entitlement(buyer, chapter_id, None)
            .await
            .unwrap(),
        EntitlementDecision::Allowed { .. }
    ));

    // C's shared database gate must agree with D's purchased-entitlement port.
    let canonical_readable: bool = sqlx::query_scalar(
        "SELECT tor_purchase_version_readable($1,$2,$3,$4)",
    )
    .bind(actor)
    .bind(
        sqlx::query_scalar::<_, i64>(
            "SELECT purchase_id FROM library_purchased_chapters WHERE user_id=$1 AND chapter_id=$2",
        )
        .bind(actor)
        .bind(chapter)
        .fetch_one(&pool)
        .await
        .unwrap(),
    )
    .bind(chapter)
    .bind(version)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(canonical_readable);

    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        rights
            .resolve_entitlement(buyer, chapter_id, None)
            .await
            .unwrap(),
        denied(DenialReason::Locked)
    );
    sqlx::query("UPDATE chapters SET admin_hold=false,is_deleted=true WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        rights
            .resolve_entitlement(buyer, chapter_id, None)
            .await
            .unwrap(),
        denied(DenialReason::Hidden)
    );
    sqlx::query("UPDATE chapters SET is_deleted=false WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        rights
            .resolve_entitlement(buyer, chapter_id, None)
            .await
            .unwrap(),
        denied(DenialReason::Locked)
    );
}

/// D-owned test fixture, not an economics mutation API. The B-10 PG suite
/// separately tests owner authorization and revision CAS in tor-books.
async fn set_fixture_economics_revision(
    pool: &PgPool,
    book: i64,
    actor: i64,
    expected: i64,
    recipients: &[(i64, i16)],
) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM books WHERE id=$1 FOR UPDATE")
        .bind(book)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO revenue_share_versions
         (book_id,revision,changed_by_user_id,reason)
         VALUES($1,$2,$3,'checkout fixture transition')",
    )
    .bind(book)
    .bind(expected + 1)
    .bind(actor)
    .execute(&mut *tx)
    .await
    .unwrap();
    for &(user_id, percent) in recipients {
        sqlx::query(
            "INSERT INTO revenue_share_assignments
             (book_id,revision,user_id,share_percent)
             VALUES($1,$2,$3,$4)",
        )
        .bind(book)
        .bind(expected + 1)
        .bind(user_id)
        .bind(percent)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE revenue_share_states SET current_revision=$2
         WHERE book_id=$1 AND current_revision=$3",
    )
    .bind(book)
    .bind(expected + 1)
    .bind(expected)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn b_10_pg_paid_purchase_freezes_share_revision_and_exact_member_percentages() {
    let (pool, buyer, book, chapter, version) = fixture("share_snapshot", 900, 100).await;
    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    let editor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('b10-commerce-editor@example.test','B10 editor','$argon2id$ci') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',TRUE)",
    )
    .bind(book)
    .bind(editor)
    .execute(&pool)
    .await
    .unwrap();

    set_fixture_economics_revision(&pool, book, owner, 1, &[(owner, 80), (editor, 20)]).await;

    let checkout = PgCheckout::new(pool.clone());
    let command_one = command(buyer, chapter, version, 100, 'A');
    let first = checkout.checkout_once(&command_one).await.unwrap();
    assert_eq!(first.balance_after.get(), 800);
    let (pinned_revision, gross): (i64, i64) = sqlx::query_as(
        "SELECT share_revision,gross_amount_minor
         FROM chapter_purchase_share_snapshots WHERE purchase_id=$1",
    )
    .bind(first.purchase_id.get())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((pinned_revision, gross), (2, 100));
    let old_lines: Vec<(i64, i16)> = sqlx::query_as(
        "SELECT user_id,share_percent FROM chapter_purchase_share_lines
         WHERE purchase_id=$1 ORDER BY user_id",
    )
    .bind(first.purchase_id.get())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(old_lines.len(), 2);
    assert!(old_lines.contains(&(owner, 80)));
    assert!(old_lines.contains(&(editor, 20)));

    set_fixture_economics_revision(&pool, book, owner, 2, &[(owner, 100)]).await;
    assert_eq!(
        checkout
            .checkout_once(&command_one)
            .await
            .unwrap()
            .purchase_id,
        first.purchase_id,
        "historical idempotent receipt must ignore later share revision"
    );
    let persisted: Vec<(i64, i16)> = sqlx::query_as(
        "SELECT user_id,share_percent FROM chapter_purchase_share_lines
         WHERE purchase_id=$1 ORDER BY user_id",
    )
    .bind(first.purchase_id.get())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, old_lines);
    assert!(
        sqlx::query(
            "UPDATE chapter_purchase_share_snapshots SET share_revision=3
             WHERE purchase_id=$1",
        )
        .bind(first.purchase_id.get())
        .execute(&pool)
        .await
        .is_err(),
        "old purchase must not be reassigned to current split"
    );
    assert!(
        sqlx::query("DELETE FROM chapter_purchase_share_lines WHERE purchase_id=$1")
            .bind(first.purchase_id.get())
            .execute(&pool)
            .await
            .is_err(),
        "historical split cannot be removed"
    );

    let (chapter_two, version_two) = chapter_with_offer(&pool, book, 2, 100).await;
    let second = checkout
        .checkout_once(&command(buyer, chapter_two, version_two, 100, 'B'))
        .await
        .unwrap();
    assert_eq!(second.balance_after.get(), 700);
    let fresh: (i64, i16) = sqlx::query_as(
        "SELECT s.share_revision,l.share_percent
         FROM chapter_purchase_share_snapshots s
         JOIN chapter_purchase_share_lines l ON l.purchase_id=s.purchase_id
         WHERE s.purchase_id=$1 AND l.user_id=$2",
    )
    .bind(second.purchase_id.get())
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fresh, (3, 100));
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn b_10_pg_legacy_unknown_economics_denies_paid_checkout_without_debit() {
    let (pool, buyer, _, _, _) = fixture("unverified_sale", 900, 100).await;
    let legacy_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:b10:unknown:commerce','ru','Unknown old economics','public')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let (chapter, version) = chapter_with_offer(&pool, legacy_book, 1, 100).await;
    let r = PgCheckout::new(pool.clone())
        .checkout_once(&command(buyer, chapter, version, 100, 'C'))
        .await;
    assert_eq!(r, Err(CheckoutError::Inaccessible));
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(balance, 900);
    let purchases: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chapter_purchase_facts WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(purchases, 0);
    pool.close().await;
}
#[path = "slices_pg/d06_direct_exposure.rs"]
mod d06_direct_exposure;
#[path = "slices_pg/d06_refund_requests.rs"]
mod d06_refund_requests;
#[path = "slices_pg/d08_revenue_allocation.rs"]
mod d08_revenue_allocation;
#[path = "slices_pg/d06_refund_approval.rs"]
mod d06_refund_approval;
