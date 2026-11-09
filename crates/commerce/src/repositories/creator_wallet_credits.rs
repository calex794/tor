//! S038 / FIN-02b: idempotent, source-backed credit of mature creator
//! earnings to the same spendable RUB wallet used for chapter purchases.
//! No external transfer, bank approval, scheduled job or public endpoint.
#![forbid(unsafe_code)]

use sqlx::PgPool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreatorWalletCreditError {
    Invalid,
    Forbidden,
    Unavailable,
}

#[derive(Clone)]
pub struct PgCreatorWalletCredits {
    pool: PgPool,
}

impl PgCreatorWalletCredits {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Process at most 20 source earnings for the current wallet owner.
    /// It is safe for several app replicas to call this with the same actor:
    /// a wallet row lock serializes selection and the (purchase,user) PK
    /// provides a second, immutable exactly-once authority.
    ///
    /// This operation is intentionally not HTTP-mounted until S038/D-06
    /// financial integration and operator policies are accepted.
    pub async fn settle_mature_once(
        &self,
        actor: i64,
        limit: u16,
    ) -> Result<u16, CreatorWalletCreditError> {
        if actor <= 0 || !(1..=20).contains(&limit) {
            return Err(CreatorWalletCreditError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CreatorWalletCreditError::Unavailable)?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM users
                 WHERE id=$1 AND account_state='active'
             )",
        )
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CreatorWalletCreditError::Unavailable)?;
        if !active {
            return Err(CreatorWalletCreditError::Forbidden);
        }
        let has_candidate: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM finance_creator_earnings e
                WHERE e.user_id=$1
                  AND e.origin_type='chapter_purchase'
                  AND e.clear_at<=clock_timestamp()
                  AND NOT EXISTS (
                    SELECT 1 FROM finance_creator_wallet_credits c
                    WHERE c.purchase_id=e.purchase_id AND c.user_id=e.user_id
                  )
                  AND NOT EXISTS (
                    SELECT 1 FROM finance_direct_refund_requests q
                    WHERE q.purchase_id=e.purchase_id
                      AND q.state IN ('pending','processing','approved')
                  )
                  AND NOT EXISTS (
                    SELECT 1 FROM finance_direct_refund_creator_reversals r
                    WHERE r.purchase_id=e.purchase_id AND r.user_id=e.user_id
                  )
                  AND NOT EXISTS (
                    SELECT 1 FROM finance_direct_swap_creator_reversals sr
                    WHERE sr.source_purchase_id=e.purchase_id AND sr.user_id=e.user_id
                  )
            )",
        )
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CreatorWalletCreditError::Unavailable)?;
        if !has_candidate {
            return Ok(0);
        }

        // The wallet row lock is also used by ordinary single/batch checkout
        // and refund credit. Never create a double-spend window in this path.
        sqlx::query(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES($1,0,'RUB') ON CONFLICT(user_id) DO NOTHING",
        )
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(|_| CreatorWalletCreditError::Unavailable)?;
        let _: i64 = sqlx::query_scalar(
            "SELECT balance_minor FROM reader_wallet_accounts
             WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
        )
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CreatorWalletCreditError::Unavailable)?;

        // This second query is evaluated AFTER the wallet lock: a concurrent
        // settlement has already committed, and any refund holds are current.
        let sources: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT e.purchase_id,e.amount_minor
             FROM finance_creator_earnings e
             JOIN chapter_purchase_facts p ON p.id=e.purchase_id
             WHERE e.user_id=$1 AND e.origin_type='chapter_purchase'
               AND e.clear_at<=clock_timestamp()
               AND p.refund_deadline_at<=clock_timestamp()
               AND NOT EXISTS (
                 SELECT 1 FROM finance_creator_wallet_credits c
                 WHERE c.purchase_id=e.purchase_id AND c.user_id=e.user_id
               )
               AND NOT EXISTS (
                 SELECT 1 FROM finance_direct_refund_requests q
                 WHERE q.purchase_id=e.purchase_id
                   AND q.state IN ('pending','processing','approved')
               )
               AND NOT EXISTS (
                 SELECT 1 FROM finance_direct_refund_creator_reversals r
                 WHERE r.purchase_id=e.purchase_id AND r.user_id=e.user_id
               )
               AND NOT EXISTS (
                 SELECT 1 FROM finance_direct_swap_creator_reversals sr
                 WHERE sr.source_purchase_id=e.purchase_id AND sr.user_id=e.user_id
               )
             ORDER BY e.purchase_id LIMIT $2 FOR UPDATE OF p,e",
        )
        .bind(actor)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| CreatorWalletCreditError::Unavailable)?;
        for (purchase_id, amount_minor) in &sources {
            let balance: i64 = sqlx::query_scalar(
                "UPDATE reader_wallet_accounts
                 SET balance_minor=balance_minor+$2
                 WHERE user_id=$1 AND currency='RUB'
                 RETURNING balance_minor",
            )
            .bind(actor)
            .bind(*amount_minor)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| CreatorWalletCreditError::Unavailable)?;
            let reference = format!("creator-credit:{purchase_id}:{actor}");
            let entry: i64 = sqlx::query_scalar(
                "INSERT INTO reader_wallet_entries
                 (user_id,kind,delta_minor,balance_after_minor,reference)
                 VALUES($1,'creator_earning_credit',$2,$3,$4) RETURNING id",
            )
            .bind(actor)
            .bind(*amount_minor)
            .bind(balance)
            .bind(reference)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| CreatorWalletCreditError::Unavailable)?;
            sqlx::query(
                "INSERT INTO finance_creator_wallet_credits
                 (purchase_id,user_id,amount_minor,wallet_entry_id)
                 VALUES($1,$2,$3,$4)",
            )
            .bind(*purchase_id)
            .bind(actor)
            .bind(*amount_minor)
            .bind(entry)
            .execute(&mut *tx)
            .await
            .map_err(|_| CreatorWalletCreditError::Unavailable)?;
        }
        tx.commit()
            .await
            .map_err(|_| CreatorWalletCreditError::Unavailable)?;
        u16::try_from(sources.len()).map_err(|_| CreatorWalletCreditError::Unavailable)
    }
}
