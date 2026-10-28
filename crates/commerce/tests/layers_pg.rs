//! D-05: real PostgreSQL contract; isolated tor_ci database only.
use sqlx::PgPool;
use tor_commerce::{
    domain::{
        layers::{LayerCheckoutCommand, LayerError},
        money::{IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::layers::PgLayers,
};
use tor_contracts::paid_delivery::PositiveId;

fn id(value: i64) -> PositiveId {
    PositiveId::new(value).unwrap()
}
fn cmd(
    buyer: i64,
    layer: i64,
    offer: i64,
    version: i64,
    price: i64,
    key: char,
) -> LayerCheckoutCommand {
    LayerCheckoutCommand {
        actor: id(buyer),
        wallet: WalletId::new(buyer).unwrap(),
        layer: id(layer),
        offer: id(offer),
        version: id(version),
        expected_price: MoneyMinor::new(price).unwrap(),
        key: IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
    }
}
async fn setup(suffix: &str) -> (PgPool, PgLayers, i64, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("tor_ci required");
    assert!(
        url.ends_with("/tor_ci"),
        "never run financial tests on production"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let mut ids = Vec::new();
    for role in ["owner", "buyer"] {
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'D05 test','$argon2id$test') RETURNING id",
        )
        .bind(format!("d05-{role}-{suffix}@example.test"))
        .fetch_one(&pool)
        .await
        .unwrap();
        ids.push(user);
    }
    let owner = ids[0];
    let buyer = ids[1];
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
                 VALUES($1,1500,'RUB')",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','D05 own book','public',$2) RETURNING id",
    )
    .bind(format!("ci:d05:{suffix}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgLayers::new(pool.clone());
    (pool, repo, owner, buyer, book)
}
async fn offer(
    repo: &PgLayers,
    owner: i64,
    book: i64,
    price: i64,
    lifetime: bool,
) -> (i64, i64, i64) {
    let layer = repo
        .create_layer(id(owner), "Пакет из своих книг")
        .await
        .unwrap();
    let version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let offer = repo
        .publish_offer(id(owner), id(layer), price, lifetime)
        .await
        .unwrap();
    (layer, version, offer)
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_purchase_replay_and_next_term_do_not_double_charge() {
    let (pool, repo, owner, buyer, book) = setup("replay").await;
    let (layer, version, offer) = offer(&repo, owner, book, 250, false).await;
    let buy = cmd(buyer, layer, offer, version, 250, 'A');
    let one = repo.buy(&buy).await.unwrap();
    let again = repo.buy(&buy).await.unwrap();
    assert_eq!(one.term_id, again.term_id);
    assert_eq!(one.balance_after_minor, 1250);
    assert!(again.replayed);
    let term_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commerce_layer_terms WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(term_count, 1);
    assert_eq!(
        repo.buy(&cmd(buyer, layer, offer, version, 251, 'A')).await,
        Err(LayerError::ReplayConflict)
    );
    let next = repo
        .buy(&cmd(buyer, layer, offer, version, 250, 'B'))
        .await
        .unwrap();
    assert_ne!(one.term_id, next.term_id);
    assert_eq!(next.balance_after_minor, 1000);
    let (stacked, thirty): (bool, bool) = sqlx::query_as(
        "SELECT t2.started_at=t1.expires_at,
                t2.expires_at=t1.expires_at+INTERVAL '30 days'
         FROM commerce_layer_terms t1 JOIN commerce_layer_terms t2 ON t2.id=$2
         WHERE t1.id=$1",
    )
    .bind(one.term_id)
    .bind(next.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        stacked,
        "30 days of renewal must begin at the previous term end"
    );
    assert!(thirty, "30 paid days must be appended in full");
    let ledger_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_wallet_entries
         WHERE user_id=$1 AND kind='layer_purchase'",
    )
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ledger_count, 2);
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_removed_books_are_accumulated_across_continuous_renewals() {
    let (pool, repo, owner, buyer, book) = setup("version").await;
    let (layer, version, offer) = offer(&repo, owner, book, 200, false).await;
    let first = repo
        .buy(&cmd(buyer, layer, offer, version, 200, 'A'))
        .await
        .unwrap();
    let new_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d05:second:version','ru','Second own','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let new_version = repo
        .add_book(id(owner), id(layer), id(new_book))
        .await
        .unwrap();
    let removed_version = repo
        .remove_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    assert_ne!(new_version, removed_version);
    // Removed from the offer, retained by the immutable earlier version.
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
    assert!(repo.can_read_book(id(buyer), id(new_book)).await.unwrap());

    // B-10 economics may change after removal from the CURRENT composition.
    // Paid subscriber history must retain the original 100% owner revision;
    // renewal snapshots use rev2 of the inherited old Book and rev1 of new.
    let collaborator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('d05-b10-collaborator@example.test','D05 collaborator','$argon2id$ci')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',TRUE)",
    )
    .bind(book)
    .bind(collaborator)
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM books WHERE id=$1 FOR UPDATE")
        .bind(book)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO revenue_share_versions
         (book_id,revision,changed_by_user_id,reason)
         VALUES($1,2,$2,'layer purchase regression')",
    )
    .bind(book)
    .bind(owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (recipient, percentage) in [(owner, 80_i16), (collaborator, 20_i16)] {
        sqlx::query(
            "INSERT INTO revenue_share_assignments
             (book_id,revision,user_id,share_percent) VALUES($1,2,$2,$3)",
        )
        .bind(book)
        .bind(recipient)
        .bind(percentage)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE revenue_share_states SET current_revision=2 WHERE book_id=$1")
        .bind(book)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let second = repo
        .buy(&cmd(buyer, layer, offer, removed_version, 200, 'B'))
        .await
        .unwrap();
    let first_revisions: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT book_id,share_revision
         FROM commerce_layer_term_share_snapshots WHERE term_id=$1 ORDER BY book_id",
    )
    .bind(first.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        first_revisions,
        vec![(book, 1)],
        "first purchase froze rev1"
    );
    let second_revisions: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT book_id,share_revision
         FROM commerce_layer_term_share_snapshots WHERE term_id=$1 ORDER BY book_id",
    )
    .bind(second.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut expected = vec![(book, 2), (new_book, 1)];
    expected.sort();
    assert_eq!(second_revisions, expected, "renewal freezes old+new books");
    let first_split: Vec<(i64, i16)> = sqlx::query_as(
        "SELECT user_id,share_percent FROM commerce_layer_term_share_lines
         WHERE term_id=$1 AND book_id=$2",
    )
    .bind(first.term_id)
    .bind(book)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(first_split, vec![(owner, 100)]);
    let second_split: Vec<(i64, i16)> = sqlx::query_as(
        "SELECT user_id,share_percent FROM commerce_layer_term_share_lines
         WHERE term_id=$1 AND book_id=$2 ORDER BY user_id",
    )
    .bind(second.term_id)
    .bind(book)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(second_split.len(), 2);
    assert!(second_split.contains(&(owner, 80)));
    assert!(second_split.contains(&(collaborator, 20)));
    assert_eq!(
        repo.buy(&cmd(buyer, layer, offer, version, 200, 'A'))
            .await
            .unwrap()
            .term_id,
        first.term_id,
        "replay keeps the original rev1 provenance and wallet settlement"
    );
    assert!(
        sqlx::query(
            "UPDATE commerce_layer_term_share_snapshots SET share_revision=2
             WHERE term_id=$1 AND book_id=$2",
        )
        .bind(first.term_id)
        .bind(book)
        .execute(&pool)
        .await
        .is_err(),
        "historical Layer finance provenance cannot be retargeted"
    );
    let (contiguous, inherited): (bool, bool) = sqlx::query_as(
        "SELECT p2.started_at=p1.expires_at,
                p2.access_origin_version_id=p1.access_origin_version_id
         FROM commerce_layer_terms p1 JOIN commerce_layer_terms p2 ON p2.id=$2
         WHERE p1.id=$1",
    )
    .bind(first.term_id)
    .bind(second.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(contiguous, "early renewal must not steal paid days");
    assert!(
        inherited,
        "renewal must keep the initial book even after removal"
    );
    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'refund','d05-version-reversal')",
    )
    .bind(first.term_id)
    .execute(&pool)
    .await
    .unwrap();
    let (term2_exists, term2_not_revoked): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM commerce_layer_terms WHERE id=$1),
                NOT EXISTS(SELECT 1 FROM commerce_layer_term_revocations WHERE term_id=$1)",
    )
    .bind(second.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(term2_exists && term2_not_revoked);
    // P2 is paid but starts only when the P1 period would have ended.
    assert!(!repo.can_read_book(id(buyer), id(book)).await.unwrap());
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_lifetime_retains_future_versions_after_removal() {
    let (pool, repo, owner, buyer, book) = setup("lifetime").await;
    let (layer, version, offer) = offer(&repo, owner, book, 200, true).await;
    let receipt = repo
        .buy(&cmd(buyer, layer, offer, version, 200, 'A'))
        .await
        .unwrap();
    let added: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d05:second:lifetime','ru','Lifetime B','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    repo.add_book(id(owner), id(layer), id(added))
        .await
        .unwrap();
    repo.remove_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
    assert!(repo.can_read_book(id(buyer), id(added)).await.unwrap());
    assert_eq!(
        repo.buy(&cmd(buyer, layer, offer, version, 200, 'B')).await,
        Err(LayerError::StaleOffer)
    );
    let term_kind: String =
        sqlx::query_scalar("SELECT term_kind FROM commerce_layer_terms WHERE id=$1")
            .bind(receipt.term_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(term_kind, "lifetime");
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_expired_term_and_wrong_owner_are_denied() {
    let (pool, repo, owner, buyer, book) = setup("expiry").await;
    let (layer, version, offer) = offer(&repo, owner, book, 200, false).await;
    let term = repo
        .buy(&cmd(buyer, layer, offer, version, 200, 'A'))
        .await
        .unwrap();
    assert_eq!(
        repo.add_book(id(buyer), id(layer), id(book)).await,
        Err(LayerError::Inaccessible)
    );
    assert_eq!(
        repo.buy(&cmd(owner, layer, offer, version, 200, 'B')).await,
        Err(LayerError::Inaccessible)
    );
    // The end time is edited only in the disposable fixture, never via production API.
    sqlx::query(
        "UPDATE commerce_layer_terms SET expires_at=now()-INTERVAL '1 second'
                 WHERE id=$1",
    )
    .bind(term.term_id)
    .execute(&pool)
    .await
    .expect_err("immutable term history must reject UPDATE");
    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block','d05-expiry-legal-block')",
    )
    .bind(term.term_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!repo.can_read_book(id(buyer), id(book)).await.unwrap());
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_owner_transfer_guard_covers_live_and_historical_subscribers() {
    let (pool, repo, owner, buyer, book) = setup("owner-fence").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 200, false).await;

    // Live composition prevents both silent ownership change and onboarding
    // an additional creator, even before the first paid term.
    assert!(sqlx::query("UPDATE books SET owner_user_id=$2 WHERE id=$1")
        .bind(book)
        .bind(buyer)
        .execute(&pool)
        .await
        .is_err());
    assert!(sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role)
             VALUES($1,$2,'translator')",
    )
    .bind(book)
    .bind(buyer)
    .execute(&pool)
    .await
    .is_err());

    let bought = repo
        .buy(&cmd(buyer, layer, offer_id, version, 200, 'A'))
        .await
        .unwrap();
    let renewed = repo
        .buy(&cmd(buyer, layer, offer_id, version, 200, 'B'))
        .await
        .unwrap();
    let renewal_is_future: bool =
        sqlx::query_scalar("SELECT started_at>now() FROM commerce_layer_terms WHERE id=$1")
            .bind(renewed.term_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(renewal_is_future, "second paid term has not started yet");
    repo.remove_book(id(owner), id(layer), id(book))
        .await
        .unwrap();

    // Current package no longer lists this Book, but immutable V1 rights
    // still entitle this subscriber. Transfer must remain blocked.
    let still_live: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM commerce_layers l
             JOIN commerce_layer_version_books vb
               ON vb.layer_version_id=l.current_version_id
             WHERE vb.book_id=$1)",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!still_live);
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
    assert!(
        sqlx::query("UPDATE books SET owner_user_id=$2 WHERE id=$1")
            .bind(book)
            .bind(buyer)
            .execute(&pool)
            .await
            .is_err(),
        "version-derived active entitlement blocks owner change"
    );

    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block','d05-owner-fence-legal-block')",
    )
    .bind(bought.term_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!repo.can_read_book(id(buyer), id(book)).await.unwrap());
    assert!(
        sqlx::query("UPDATE books SET owner_user_id=$2 WHERE id=$1")
            .bind(book)
            .bind(buyer)
            .execute(&pool)
            .await
            .is_err(),
        "a prepaid future renewal is an outstanding historical Book obligation"
    );

    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block','d05-owner-fence-renewal-block')",
    )
    .bind(renewed.term_id)
    .execute(&pool)
    .await
    .unwrap();

    // After revocation and removal, no Layer right remains. A complete
    // B-09 transfer still needs to swap members atomically; this manual SQL
    // probe is rolled back to avoid creating a mismatched owner/member.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE books SET owner_user_id=$2 WHERE id=$1")
        .bind(book)
        .bind(buyer)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_layer_requires_exactly_one_active_owner_member() {
    let (pool, repo, owner, buyer, book) = setup("sole-owner").await;
    let layer = repo
        .create_layer(id(owner), "Layer ownership invariant")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',TRUE)",
    )
    .bind(book)
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.add_book(id(owner), id(layer), id(book)).await,
        Err(LayerError::Inaccessible),
        "Book with other active creator cannot enter Layer"
    );
    sqlx::query(
        "UPDATE book_members SET is_active=FALSE
         WHERE book_id=$1 AND user_id=$2",
    )
    .bind(book)
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let _version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    assert!(
        sqlx::query(
            "UPDATE book_members SET is_active=TRUE
             WHERE book_id=$1 AND user_id=$2",
        )
        .bind(book)
        .bind(buyer)
        .execute(&pool)
        .await
        .is_err(),
        "member reactivation cannot evade Layer ownership lock"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_layer_unknown_revenue_schedule_cannot_debit() {
    let (pool, repo, owner, buyer, book) = setup("unknown-economics").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 250, false).await;

    // Simulate a legacy/imported Book whose economic configuration has not
    // been attested. Never replace missing data with fabricated owner 100%.
    sqlx::query("DELETE FROM revenue_share_states WHERE book_id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        repo.buy(&cmd(buyer, layer, offer_id, version, 250, 'A'))
            .await,
        Err(LayerError::Inaccessible)
    );
    let wallet: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(wallet, 1500);
    let terms: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM commerce_layer_terms WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    let commands: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM commerce_layer_commands WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((terms, commands), (0, 0), "no partial financial commit");
    pool.close().await;
}
