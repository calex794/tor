//! D-05/D-08: prove a non-posting attribution reads the paid term's
//! frozen B-10 schedules, not the current Layer composition.
use super::*;
use tor_commerce::domain::layer_allocation::{
    preview_layer_attribution, AttributionError, AttributionInputs, BookWeighting, FrozenBook,
    FrozenParticipant,
};

async fn frozen_rows(pool: &PgPool, term: i64) -> Vec<(i64, i64, i64, i16)> {
    sqlx::query_as(
        "SELECT s.book_id,s.share_revision,l.user_id,l.share_percent
         FROM commerce_layer_term_share_snapshots s
         JOIN commerce_layer_term_share_lines l
           ON l.term_id=s.term_id AND l.book_id=s.book_id
         WHERE s.term_id=$1 ORDER BY s.book_id,l.user_id",
    )
    .bind(term)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_layer_equal_minor_preview_preserves_paid_books_after_future_additions() {
    let (pool, repo, owner, buyer, original_book) = setup("allocation-preview").await;
    let (layer, _v1, _) = offer(&repo, owner, original_book, 101, false).await;
    let second_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','D05 second paid Book','public',$2) RETURNING id",
    )
    .bind(format!("ci:d05:allocation-second:{original_book}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version = repo
        .add_book(id(owner), id(layer), id(second_book))
        .await
        .unwrap();
    let published_offer = repo
        .publish_offer(id(owner), id(layer), 101, false)
        .await
        .unwrap();

    let command = cmd(buyer, layer, published_offer, paid_version, 101, 'A');
    let receipt = repo.buy(&command).await.unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.balance_after_minor, 1399);

    let snapshot = frozen_rows(&pool, receipt.term_id).await;
    assert_eq!(snapshot.len(), 2);
    assert!(snapshot
        .iter()
        .all(|row| row.1 == 1 && row.2 == owner && row.3 == 100));
    let books = snapshot
        .iter()
        .map(|&(book_id, share_revision, user_id, percent)| FrozenBook {
            book_id,
            share_revision,
            weight_bps: None,
            participants: vec![FrozenParticipant {
                user_id,
                percent: u16::try_from(percent).unwrap(),
            }],
        })
        .collect();
    // A test-approved, zero-fee arithmetic example is NOT authority to
    // recognize cash earnings or to enable payout endpoints.
    let inputs = AttributionInputs {
        payment_id: receipt.term_id,
        term_id: receipt.term_id,
        gross_minor: 101,
        currency: "RUB".into(),
        policy_id: Some("ci-non-posting-legacy-equal-v1".into()),
        platform_fee_minor: Some(0),
        weighting: Some(BookWeighting::LegacyEqualLast),
        frozen_schedule_verified: true,
        allocation_basis_confirmed: true,
        books,
    };
    let preview = preview_layer_attribution(&inputs).unwrap();
    assert_eq!(preview.attributed_minor(), 101);
    assert_eq!(
        preview
            .books
            .iter()
            .map(|b| b.amount_minor)
            .collect::<Vec<_>>(),
        vec![50, 51]
    );
    assert_eq!(
        preview
            .books
            .iter()
            .map(|b| b.share_revision)
            .collect::<Vec<_>>(),
        vec![1, 1]
    );

    // A future Book enters the live Layer and the reader's CURRENT rights,
    // but an old paid term must not silently fabricate new money lines.
    let future_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','D05 future Book','public',$2) RETURNING id",
    )
    .bind(format!("ci:d05:allocation-future:{original_book}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    repo.add_book(id(owner), id(layer), id(future_book))
        .await
        .unwrap();
    assert!(repo
        .can_read_book(id(buyer), id(future_book))
        .await
        .unwrap());
    assert_eq!(
        frozen_rows(&pool, receipt.term_id).await,
        snapshot,
        "historical Book snapshots cannot grow with future versions"
    );
    // A removed historical Book may later gain a translator and B-10 rev2.
    // That change must never reprice the existing term's frozen rev1.
    repo.remove_book(id(owner), id(layer), id(original_book))
        .await
        .unwrap();
    assert!(repo
        .can_read_book(id(buyer), id(original_book))
        .await
        .unwrap());
    let translator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D05 allocation translator','$argon2id$test') RETURNING id",
    )
    .bind(format!(
        "d05-allocation-translator-{original_book}@example.test"
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',TRUE)",
    )
    .bind(original_book)
    .bind(translator)
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM books WHERE id=$1 FOR UPDATE")
        .bind(original_book)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO revenue_share_versions
         (book_id,revision,changed_by_user_id,reason)
         VALUES($1,2,$2,'historical Layer allocation regression')",
    )
    .bind(original_book)
    .bind(owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (recipient, percent) in [(owner, 70_i16), (translator, 30_i16)] {
        sqlx::query(
            "INSERT INTO revenue_share_assignments
             (book_id,revision,user_id,share_percent) VALUES($1,2,$2,$3)",
        )
        .bind(original_book)
        .bind(recipient)
        .bind(percent)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE revenue_share_states SET current_revision=2 WHERE book_id=$1")
        .bind(original_book)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let revision: i64 =
        sqlx::query_scalar("SELECT current_revision FROM revenue_share_states WHERE book_id=$1")
            .bind(original_book)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(revision, 2);
    assert_eq!(frozen_rows(&pool, receipt.term_id).await, snapshot);
    assert_eq!(preview_layer_attribution(&inputs).unwrap(), preview);
    let replay = repo.buy(&command).await.unwrap();
    assert_eq!(replay.term_id, receipt.term_id);
    assert!(replay.replayed);
    let debits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_wallet_entries
         WHERE user_id=$1 AND layer_term_id=$2 AND kind='layer_purchase'",
    )
    .bind(buyer)
    .bind(receipt.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        debits, 1,
        "preview must not produce another debit or earning"
    );

    let mut unknown_policy = inputs;
    unknown_policy.platform_fee_minor = None;
    assert_eq!(
        preview_layer_attribution(&unknown_policy),
        Err(AttributionError::UnconfirmedPolicy)
    );
    pool.close().await;
}
