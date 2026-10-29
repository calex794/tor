//! D-06: actor-bound, non-posting observations about paid protected-content use.
//! These are NOT eligibility decisions and never prove that content was unused.
//! A single SQL SELECT returns a consistent MVCC snapshot of immutable events.
//! Any future refund decision MUST reread evidence under the purchase/term lock
//! in its own SERIALIZABLE money/revocation transaction.
use crate::services::refunds::{RefundGateError, UseEvidence, UseObservation};
use sqlx::PgPool;
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone)]
pub struct PgRefundUseEvidence {
    pool: PgPool,
}

impl PgRefundUseEvidence {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Purchase-specific direct chapter evidence. Never fall back to Layer,
    /// reader progress or public chapter publication. The immutable wallet
    /// debit and frozen share snapshot are checked in the SAME SELECT.
    pub async fn direct(
        &self,
        actor: PositiveId,
        purchase: PositiveId,
    ) -> Result<UseObservation, RefundGateError> {
        let flags: Option<(bool, bool, bool, bool)> = sqlx::query_as(
            "SELECT
                EXISTS (
                    SELECT 1 FROM chapter_purchase_render_confirmations a
                    WHERE a.purchase_id=p.id AND a.user_id=p.user_id
                      AND a.chapter_id=p.chapter_id AND a.version_id=p.version_id
                ),
                EXISTS (
                    SELECT 1 FROM chapter_purchase_text_grants g
                    WHERE g.purchase_id=p.id AND g.user_id=p.user_id
                      AND g.chapter_id=p.chapter_id AND g.version_id=p.version_id
                ),
                EXISTS (
                    SELECT 1 FROM chapter_purchase_first_exposures f
                    WHERE f.purchase_id=p.id AND f.user_id=p.user_id
                      AND f.chapter_id=p.chapter_id AND f.version_id=p.version_id
                ),
                EXISTS (
                    SELECT 1 FROM chapter_purchase_revocations r
                    WHERE r.purchase_id=p.id
                )
             FROM chapter_purchase_facts p
             WHERE p.id=$1 AND p.user_id=$2
               AND p.amount_minor>0 AND p.currency='RUB'
               AND EXISTS (
                   SELECT 1 FROM reader_wallet_entries e
                   WHERE e.purchase_id=p.id AND e.user_id=p.user_id
                     AND e.kind='chapter_purchase' AND e.delta_minor=-p.amount_minor
               )
               AND EXISTS (
                   SELECT 1 FROM chapter_purchase_share_snapshots s
                   WHERE s.purchase_id=p.id AND s.gross_amount_minor=p.amount_minor
               )",
        )
        .bind(purchase.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RefundGateError::Unavailable)?;
        Self::classify(flags)
    }

    /// Term-specific Layer evidence. In particular, the first grant for P1
    /// must never mark a future, separately paid renewal P2 as opened.
    /// Existing revocation is an access fact, not proof of money returned.
    pub async fn layer(
        &self,
        actor: PositiveId,
        term: PositiveId,
    ) -> Result<UseObservation, RefundGateError> {
        let flags: Option<(bool, bool, bool, bool)> = sqlx::query_as(
            "SELECT
                EXISTS (
                    SELECT 1 FROM commerce_layer_term_render_confirmations a
                    WHERE a.term_id=t.id
                ),
                EXISTS (
                    SELECT 1 FROM commerce_layer_term_text_grants g
                    WHERE g.term_id=t.id
                ),
                EXISTS (
                    SELECT 1 FROM commerce_layer_term_first_exposures f
                    WHERE f.term_id=t.id
                ),
                EXISTS (
                    SELECT 1 FROM commerce_layer_term_revocations r
                    WHERE r.term_id=t.id
                )
             FROM commerce_layer_terms t
             WHERE t.id=$1 AND t.user_id=$2
               AND d05_layer_term_has_wallet_evidence(t.id)",
        )
        .bind(term.get())
        .bind(actor.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RefundGateError::Unavailable)?;
        Self::classify(flags)
    }

    fn classify(
        flags: Option<(bool, bool, bool, bool)>,
    ) -> Result<UseObservation, RefundGateError> {
        let (rendered, text_granted, grant_issued, revoked) =
            flags.ok_or(RefundGateError::NotFound)?;
        // The FK chain makes this impossible in valid PG data. Do not treat
        // corrupted/incomplete provenance as sufficient financial evidence.
        if (rendered && !text_granted) || (text_granted && !grant_issued) {
            return Err(RefundGateError::Unavailable);
        }
        let evidence = if rendered {
            UseEvidence::BrowserRenderReported
        } else if text_granted {
            UseEvidence::ChapterTextGrantUnconfirmed
        } else if grant_issued {
            UseEvidence::ProtectedGrantOnly
        } else {
            UseEvidence::Unknown
        };
        Ok(UseObservation {
            evidence,
            access_revoked: revoked,
        })
    }
}
