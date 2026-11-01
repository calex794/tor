//! D-05/D-06 wallet CHECK compatibility: disposable PostgreSQL only.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_wallet_journal_kinds_and_provenance_are_fail_closed() {
    let (pool, repo, owner, buyer, book) = setup("wallet-compat").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 175, false).await;
    let bought = repo
        .buy(&cmd(buyer, layer, offer_id, version, 175, 'W'))
        .await
        .unwrap();

    // Actual Layer checkout must still post the immutable owner-scoped debit.
    let entry: (String, Option<i64>, Option<i64>, i64) = sqlx::query_as(
        "SELECT kind,purchase_id,layer_term_id,delta_minor
         FROM reader_wallet_entries WHERE layer_term_id=$1 AND user_id=$2",
    )
    .bind(bought.term_id)
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(entry.0, "layer_purchase");
    assert_eq!(entry.1, None);
    assert_eq!(entry.2, Some(bought.term_id));
    assert_eq!(entry.3, -175);

    // This standalone stacked PR has no D-06 refund/swap repository yet.
    // Assert the final CHECK allows all five legitimate journal kinds;
    // the eventual combined-main CI must also exercise genuine D-06 flows.
    let kinds: String = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid)
         FROM pg_constraint
         WHERE conrelid='reader_wallet_entries'::regclass
           AND conname='reader_wallet_entries_kind_check'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for kind in [
        "verified_funding",
        "chapter_purchase",
        "layer_purchase",
        "chapter_refund",
        "chapter_swap_credit",
    ] {
        assert!(
            kinds.contains(kind),
            "wallet CHECK excluded {kind}: {kinds}"
        );
    }
    let direction: String = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid)
         FROM pg_constraint
         WHERE conrelid='reader_wallet_entries'::regclass
           AND conname='wallet_entry_direction'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(direction.contains("purchase_id"));
    assert!(direction.contains("layer_term_id"));

    let balance_before: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    let count_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();

    // CHECK failures must reject fake funding, unsupported kinds, direct
    // credits without an actual chapter purchase, and a Layer debit with
    // missing historical term. All probes use independent auto-rollback SQL.
    for (index, kind, delta, purchase_id, layer_term_id) in [
        (
            1,
            "verified_funding",
            1_i64,
            None::<i64>,
            Some(bought.term_id),
        ),
        (2, "verified_funding", -1, None, None),
        (3, "chapter_purchase", -1, None, None),
        (4, "chapter_refund", 1, None, None),
        (5, "chapter_swap_credit", 1, None, None),
        (6, "layer_purchase", -1, None, None),
        (7, "unsupported_wallet_kind", 1, None, None),
    ] {
        let error = sqlx::query(
            "INSERT INTO reader_wallet_entries
             (user_id,kind,delta_minor,balance_after_minor,reference,
              purchase_id,layer_term_id)
             VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(buyer)
        .bind(kind)
        .bind(delta)
        .bind(balance_before)
        .bind(format!("ci:d05-d06:invalid:{index}"))
        .bind(purchase_id)
        .bind(layer_term_id)
        .execute(&pool)
        .await
        .expect_err("bad wallet provenance must be rejected at SQL CHECK");
        assert_eq!(
            error
                .as_database_error()
                .and_then(|db| db.code())
                .as_deref(),
            Some("23514"),
            "{kind}: {error}"
        );
    }

    let balance_after: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    let count_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(balance_after, balance_before);
    assert_eq!(count_after, count_before);
    pool.close().await;
}
