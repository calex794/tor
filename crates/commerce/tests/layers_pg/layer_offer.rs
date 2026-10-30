//! S031: public Layer price and composition are a read-only, versioned quote.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_layer_offer_quote_is_complete_and_never_authoritative_for_checkout() {
    let (pool, repo, owner, buyer, book) = setup("quote-complete").await;
    let layer = repo.create_layer(id(owner), "Layer <test>").await.unwrap();
    assert_eq!(
        repo.offer_quote(id(layer)).await,
        Err(LayerError::Inaccessible),
        "never advertise a Layer without a published offer"
    );
    let version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let offer = repo
        .publish_offer(id(owner), id(layer), 251, false)
        .await
        .unwrap();
    let first = repo.offer_quote(id(layer)).await.unwrap();
    assert_eq!(first.layer_id, layer);
    assert_eq!(first.title, "Layer <test>");
    assert_eq!(first.offer_id, offer);
    assert_eq!(first.version_id, version);
    assert_eq!(first.price_minor, 251);
    assert_eq!(
        first.term_kind,
        tor_commerce::domain::layers::LayerTermKind::Prepaid30Days
    );
    assert_eq!(first.books.len(), 1);
    assert_eq!(first.books[0].book_id, book);

    let second_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Second of two','public',$2) RETURNING id",
    )
    .bind(format!("ci:d05:quote-second:{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let revision = repo
        .add_book(id(owner), id(layer), id(second_book))
        .await
        .unwrap();
    let changed = repo.offer_quote(id(layer)).await.unwrap();
    assert_eq!(changed.version_id, revision);
    assert_eq!(changed.offer_id, offer, "offer ID does not silently change");
    assert_eq!(changed.books.len(), 2);
    assert_eq!(
        changed.books.iter().map(|b| b.book_id).collect::<Vec<_>>(),
        vec![book, second_book],
    );
    // A sale requires the latest quote; a quote never grants reading or money.
    let purchases: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commerce_layer_terms WHERE layer_id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(purchases, 0);
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.offer_quote(id(layer)).await,
        Err(LayerError::Inaccessible),
        "an incomplete or held composition must not be advertised"
    );
    assert_eq!(
        repo.buy(&cmd(buyer, layer, offer, revision, 251, 'H'))
            .await,
        Err(LayerError::Inaccessible),
        "the wallet may not be debited for a held Layer Book"
    );
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(balance, 1500);
    sqlx::query("UPDATE books SET admin_hold=FALSE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE commerce_layers SET status='blocked' WHERE id=$1")
        .bind(layer)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.offer_quote(id(layer)).await,
        Err(LayerError::Inaccessible)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_old_held_book_does_not_block_fresh_buyer_of_current_layer() {
    let (pool, repo, owner, buyer, previous) = setup("quote-old-held").await;
    let layer = repo
        .create_layer(id(owner), "Current book only")
        .await
        .unwrap();
    repo.add_book(id(owner), id(layer), id(previous))
        .await
        .unwrap();
    let offer = repo
        .publish_offer(id(owner), id(layer), 251, false)
        .await
        .unwrap();
    let current_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Current valid','public',$2) RETURNING id",
    )
    .bind(format!("ci:d05:quote-current:{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    repo.add_book(id(owner), id(layer), id(current_book))
        .await
        .unwrap();
    let current = repo
        .remove_book(id(owner), id(layer), id(previous))
        .await
        .unwrap();
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(previous)
        .execute(&pool)
        .await
        .unwrap();
    let quote = repo.offer_quote(id(layer)).await.unwrap();
    assert_eq!(quote.books.len(), 1);
    assert_eq!(quote.books[0].book_id, current_book);
    assert_eq!(quote.version_id, current);

    let paid = repo
        .buy(&cmd(buyer, layer, offer, current, 251, 'R'))
        .await
        .unwrap();
    assert_eq!(paid.balance_after_minor, 1249);
    let frozen_books: Vec<i64> = sqlx::query_scalar(
        "SELECT book_id FROM commerce_layer_term_share_snapshots WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        frozen_books,
        vec![current_book],
        "old held book is not in a new paid chain"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_owner_price_publication_is_cas_bound_and_append_only() {
    let (pool, repo, owner, buyer, book) = setup("author-offer").await;
    let layer = repo.create_layer(id(owner), "Author offer").await.unwrap();
    let empty = repo.owner_offer_state(id(owner), id(layer)).await.unwrap();
    assert_eq!(empty.current_offer_id, None);
    assert_eq!(
        repo.publish_offer_if_current(
            id(owner),
            id(layer),
            id(empty.current_version_id),
            None,
            251,
            false
        )
        .await,
        Err(LayerError::Inaccessible),
        "an empty composition has no sellable offer"
    );
    let version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    assert_eq!(
        repo.owner_offer_state(id(buyer), id(layer)).await,
        Err(LayerError::Inaccessible)
    );
    assert_eq!(
        repo.publish_offer_if_current(id(buyer), id(layer), id(version), None, 251, false)
            .await,
        Err(LayerError::Inaccessible)
    );
    let first = repo
        .publish_offer_if_current(id(owner), id(layer), id(version), None, 251, false)
        .await
        .unwrap();
    assert_eq!(
        repo.publish_offer_if_current(id(owner), id(layer), id(version), None, 251, false)
            .await,
        Err(LayerError::StaleOffer),
        "a replayed form must not silently publish another revision"
    );
    let owner_state = repo.owner_offer_state(id(owner), id(layer)).await.unwrap();
    assert_eq!(owner_state.current_offer_id, Some(first));
    assert_eq!(owner_state.price_minor, Some(251));
    let second = repo
        .publish_offer_if_current(
            id(owner),
            id(layer),
            id(version),
            Some(id(first)),
            300,
            false,
        )
        .await
        .unwrap();
    assert_ne!(first, second);
    let next = repo.owner_offer_state(id(owner), id(layer)).await.unwrap();
    assert_eq!(next.current_offer_id, Some(second));
    assert_eq!(next.price_minor, Some(300));
    let original_price: i64 =
        sqlx::query_scalar("SELECT price_minor FROM commerce_layer_offers WHERE id=$1")
            .bind(first)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(original_price, 251, "published historic offer immutable");
    let lifetime = repo
        .publish_offer_if_current(
            id(owner),
            id(layer),
            id(version),
            Some(id(second)),
            500,
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        repo.publish_offer_if_current(
            id(owner),
            id(layer),
            id(version),
            Some(id(lifetime)),
            501,
            false,
        )
        .await,
        Err(LayerError::Invalid),
        "a lifetime Layer cannot silently become monthly"
    );
    let entries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reader_wallet_entries WHERE user_id=$1 AND kind='layer_purchase'",
    )
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(entries, 0, "publishing never charges a wallet");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_owner_offer_refuses_unknown_share_schedule_and_stale_books() {
    let (pool, repo, owner, _buyer, book) = setup("author-offer-unknown").await;
    let layer = repo
        .create_layer(id(owner), "Unattested layer")
        .await
        .unwrap();
    let v1 = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    sqlx::query("DELETE FROM revenue_share_states WHERE book_id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.publish_offer_if_current(id(owner), id(layer), id(v1), None, 100, false)
            .await,
        Err(LayerError::Inaccessible),
        "a missing 100% revenue schedule must never produce a sellable offer"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM commerce_layer_offers WHERE layer_id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_republished_offer_preserves_paid_term_ledger_and_shares() {
    let (pool, repo, owner, buyer, book) = setup("author-reprice-paid").await;
    let layer = repo
        .create_layer(id(owner), "Already paid Layer")
        .await
        .unwrap();
    let version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let first_offer = repo
        .publish_offer_if_current(id(owner), id(layer), id(version), None, 251, false)
        .await
        .unwrap();
    let original_command = cmd(buyer, layer, first_offer, version, 251, 'P');
    let paid = repo.buy(&original_command).await.unwrap();
    assert_eq!(paid.balance_after_minor, 1249);

    let original_term: (i64, i64, String, i64) = sqlx::query_as(
        "SELECT offer_id,base_version_id,term_kind,amount_minor
         FROM commerce_layer_terms WHERE id=$1",
    )
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        original_term,
        (first_offer, version, "prepaid_30d".into(), 251)
    );
    let original_shares: Vec<(i64, i64, i64, i16)> = sqlx::query_as(
        "SELECT s.book_id,s.share_revision,l.user_id,l.share_percent
         FROM commerce_layer_term_share_snapshots s
         JOIN commerce_layer_term_share_lines l
           ON l.term_id=s.term_id AND l.book_id=s.book_id
         WHERE s.term_id=$1 ORDER BY s.book_id,l.user_id",
    )
    .bind(paid.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(original_shares, vec![(book, 1, owner, 100)]);

    // Repricing and switching future buyers to lifetime must not produce
    // earnings, another charge, or a silent change to this paid 30-day term.
    let next_offer = repo
        .publish_offer_if_current(
            id(owner),
            id(layer),
            id(version),
            Some(id(first_offer)),
            550,
            true,
        )
        .await
        .unwrap();
    let quote = repo.offer_quote(id(layer)).await.unwrap();
    assert_eq!(quote.offer_id, next_offer);
    assert_eq!(quote.price_minor, 550);
    assert_eq!(
        quote.term_kind,
        tor_commerce::domain::layers::LayerTermKind::Lifetime
    );
    // Paid prepaid access must not look like an already purchased lifetime.
    let before_upgrade =
        tor_commerce::repositories::layer_storefront::storefront(&pool, layer, Some(buyer))
            .await
            .unwrap()
            .unwrap();
    assert!(before_upgrade.has_access);
    assert!(!before_upgrade.has_lifetime_access);
    assert_eq!(
        repo.buy(&cmd(buyer, layer, first_offer, version, 251, 'Q'))
            .await,
        Err(LayerError::StaleOffer),
        "a distinct command cannot purchase a superseded offer"
    );
    let replay = repo.buy(&original_command).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.term_id, paid.term_id);
    assert_eq!(replay.balance_after_minor, 1249);
    let before_new_purchase: (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM commerce_layer_terms WHERE layer_id=$1),
           (SELECT COUNT(*) FROM reader_wallet_entries
            WHERE user_id=$2 AND kind='layer_purchase'),
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$2)",
    )
    .bind(layer)
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before_new_purchase, (1, 1, 1249));

    let later = repo
        .buy(&cmd(buyer, layer, next_offer, version, 550, 'R'))
        .await
        .unwrap();
    assert_ne!(later.term_id, paid.term_id);
    assert_eq!(later.balance_after_minor, 699);
    let after_upgrade =
        tor_commerce::repositories::layer_storefront::storefront(&pool, layer, Some(buyer))
            .await
            .unwrap()
            .unwrap();
    assert!(after_upgrade.has_access);
    assert!(after_upgrade.has_lifetime_access);
    let saved_term: (i64, i64, String, i64) = sqlx::query_as(
        "SELECT offer_id,base_version_id,term_kind,amount_minor
         FROM commerce_layer_terms WHERE id=$1",
    )
    .bind(paid.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved_term, original_term, "old paid terms never reprice");
    let saved_shares: Vec<(i64, i64, i64, i16)> = sqlx::query_as(
        "SELECT s.book_id,s.share_revision,l.user_id,l.share_percent
         FROM commerce_layer_term_share_snapshots s
         JOIN commerce_layer_term_share_lines l
           ON l.term_id=s.term_id AND l.book_id=s.book_id
         WHERE s.term_id=$1 ORDER BY s.book_id,l.user_id",
    )
    .bind(paid.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        saved_shares, original_shares,
        "frozen recipient shares stay frozen"
    );
    let (terms, debits): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM commerce_layer_terms WHERE layer_id=$1),
                (SELECT COUNT(*) FROM reader_wallet_entries
                 WHERE user_id=$2 AND kind='layer_purchase')",
    )
    .bind(layer)
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (terms, debits),
        (2, 2),
        "only actual purchases create paid terms"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_parallel_buy_keys_serialize_wallet_and_renewal_chain() {
    let (pool, repo, owner, buyer, book) = setup("parallel-buys").await;
    let (layer, version, published) = offer(&repo, owner, book, 250, false).await;
    let first_command = cmd(buyer, layer, published, version, 250, 'T');
    let (first, replay) = tokio::join!(repo.buy(&first_command), repo.buy(&first_command));
    let (first, replay) = (first.unwrap(), replay.unwrap());
    assert_eq!(first.term_id, replay.term_id);
    assert_eq!(first.balance_after_minor, 1250);
    assert_eq!(replay.balance_after_minor, 1250);
    assert_ne!(first.replayed, replay.replayed);

    // Two different commands are two voluntary purchases. Locking the buyer
    // wallet must serialize the renewed 30-day terms rather than overlap them.
    let second_command = cmd(buyer, layer, published, version, 250, 'U');
    let third_command = cmd(buyer, layer, published, version, 250, 'V');
    let (second, third) = tokio::join!(repo.buy(&second_command), repo.buy(&third_command),);
    let (second, third) = (second.unwrap(), third.unwrap());
    assert_ne!(first.term_id, second.term_id);
    assert_ne!(first.term_id, third.term_id);
    assert_ne!(second.term_id, third.term_id);
    assert!(!second.replayed && !third.replayed);

    let (terms, debits, balance): (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM commerce_layer_terms
            WHERE user_id=$1 AND layer_id=$2),
           (SELECT COUNT(*) FROM reader_wallet_entries
            WHERE user_id=$1 AND kind='layer_purchase'),
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1)",
    )
    .bind(buyer)
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((terms, debits, balance), (3, 3, 750));

    let continuous: bool = sqlx::query_scalar(
        "SELECT COUNT(*)=3 AND COUNT(DISTINCT access_origin_version_id)=1
                AND MAX(expires_at)=MIN(expires_at)+INTERVAL '60 days'
         FROM commerce_layer_terms WHERE user_id=$1 AND layer_id=$2",
    )
    .bind(buyer)
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        continuous,
        "parallel renewals must preserve all 90 paid days"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_concurrent_reprice_and_checkout_never_charge_a_stale_offer() {
    let (pool, repo, owner, buyer, book) = setup("reprice-race").await;
    let layer = repo.create_layer(id(owner), "Race Layer").await.unwrap();
    let version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let old_offer = repo
        .publish_offer_if_current(id(owner), id(layer), id(version), None, 250, false)
        .await
        .unwrap();
    let old_command = cmd(buyer, layer, old_offer, version, 250, 'W');

    // If checkout wins the Layer lock, the old offer may settle. If reprice
    // wins, checkout must fail closed. Neither order may debit 550 without
    // an explicit 550 quote or write to the previous paid term.
    let (published, attempted) = tokio::join!(
        repo.publish_offer_if_current(
            id(owner),
            id(layer),
            id(version),
            Some(id(old_offer)),
            550,
            false,
        ),
        repo.buy(&old_command),
    );
    let published = published.unwrap();
    let quote = repo.offer_quote(id(layer)).await.unwrap();
    assert_eq!(quote.offer_id, published);
    assert_eq!(quote.price_minor, 550);
    let accepted = match attempted {
        Ok(receipt) => {
            assert_eq!(receipt.balance_after_minor, 1250);
            assert!(!receipt.replayed);
            let frozen: (i64, i64) = sqlx::query_as(
                "SELECT offer_id,amount_minor FROM commerce_layer_terms WHERE id=$1",
            )
            .bind(receipt.term_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(frozen, (old_offer, 250));
            true
        }
        Err(LayerError::StaleOffer) => false,
        other => panic!("unexpected concurrent checkout outcome: {other:?}"),
    };
    let (terms, debits, balance): (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM commerce_layer_terms
            WHERE user_id=$1 AND layer_id=$2),
           (SELECT COUNT(*) FROM reader_wallet_entries
            WHERE user_id=$1 AND kind='layer_purchase'),
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1)",
    )
    .bind(buyer)
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (terms, debits, balance),
        if accepted { (1, 1, 1250) } else { (0, 0, 1500) }
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_legacy_public_quote_does_not_reveal_draft_books() {
    let (pool, repo, owner, _buyer, book) = setup("legacy-private-quote").await;
    let layer = repo
        .create_layer(id(owner), "Private composition")
        .await
        .unwrap();
    repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let draft: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','PRIVATE-DRAFT-BOOK','draft',$2) RETURNING id",
    )
    .bind(format!("ci:d05:legacy-private-quote:{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    repo.add_book(id(owner), id(layer), id(draft))
        .await
        .unwrap();
    repo.publish_offer(id(owner), id(layer), 125, false)
        .await
        .unwrap();
    // The legacy public quote contains unfiltered Book titles. Refuse the
    // entire quote rather than displaying a draft title to a stranger.
    assert_eq!(
        repo.offer_quote(id(layer)).await,
        Err(LayerError::Inaccessible)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_draft_only_layer_cannot_debit_wallet() {
    let (pool, repo, owner, buyer, _book) = setup("draft-only-buy").await;
    let draft: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','CI-DRAFT-ONLY','draft',$2) RETURNING id",
    )
    .bind(format!("ci:d05:draft-only-buy:{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let layer = repo.create_layer(id(owner), "Unpublished Layer").await.unwrap();
    let version = repo.add_book(id(owner), id(layer), id(draft)).await.unwrap();
    let offered = repo.publish_offer(id(owner), id(layer), 125, false).await.unwrap();

    // The public UI suppresses its buy button when no Book is published.
    // The same restriction must hold for a direct, authenticated JSON POST.
    let card = tor_commerce::repositories::layer_storefront::storefront(
        &pool, layer, Some(buyer),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(card.public_book_count, 0);
    assert!(card.public_books.is_empty());
    assert_eq!(
        repo.buy(&cmd(buyer, layer, offered, version, 125, 'Z')).await,
        Err(LayerError::Inaccessible),
    );
    let (wallet, terms, commands): (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1),
           (SELECT COUNT(*) FROM commerce_layer_terms WHERE user_id=$1),
           (SELECT COUNT(*) FROM commerce_layer_commands WHERE user_id=$1)",
    )
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((wallet, terms, commands), (1500, 0, 0));
    pool.close().await;
}
