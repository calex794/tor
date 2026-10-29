//! D-08: read-only, real-PG provenance for *non-posting* Layer math.
//! Never substitute client-provided "verified" booleans for wallet evidence.
//! Neither this repository nor its preview creates earnings, refunds or payouts.
use crate::domain::layer_allocation::{
    preview_layer_attribution, AttributionError, AttributionInputs, AttributionPreview,
    BookWeighting, FrozenBook, FrozenParticipant,
};
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerProvenanceError {
    UnverifiedPurchase,
    IncompleteFrozenSchedule,
    Unavailable,
}

#[derive(Clone, Debug, Default)]
pub struct DraftAllocationPolicy {
    /// Scenario identifier, not proof of approval by D's finance authority.
    pub policy_id: Option<String>,
    pub platform_fee_minor: Option<i64>,
    pub weighting: Option<BookWeighting>,
    /// Required only for WeightedLargestRemainder; one entry per paid Book.
    pub book_weights_bps: Vec<(i64, u16)>,
    /// A human-reviewed hypothetical scenario, never financial authority.
    pub allocation_basis_confirmed: bool,
}

/// This type can only be constructed by a successful authenticated DB read.
/// It exposes no mutable "verified" flags and is not a money posting command.
#[derive(Clone, Debug)]
pub struct VerifiedLayerProvenance {
    term_id: i64,
    gross_minor: i64,
    currency: String,
    books: Vec<FrozenBook>,
}

impl VerifiedLayerProvenance {
    /// Pure arithmetic preview. All shares come from an immutable paid term.
    /// Policy inputs are hypothetical until separate D/I authorization exists.
    pub fn preview_nonposting(
        &self,
        draft: &DraftAllocationPolicy,
    ) -> Result<AttributionPreview, AttributionError> {
        let mut books = self.books.clone();
        match draft.weighting {
            Some(BookWeighting::WeightedLargestRemainder) => {
                let mut weights = BTreeMap::new();
                for &(book_id, weight) in &draft.book_weights_bps {
                    if book_id <= 0 || weight == 0 || weights.insert(book_id, weight).is_some() {
                        return Err(AttributionError::InvalidWeights);
                    }
                }
                if weights.len() != books.len() {
                    return Err(AttributionError::InvalidWeights);
                }
                for book in &mut books {
                    book.weight_bps = Some(
                        *weights
                            .get(&book.book_id)
                            .ok_or(AttributionError::InvalidWeights)?,
                    );
                }
            }
            _ => {
                if !draft.book_weights_bps.is_empty() {
                    return Err(AttributionError::InvalidWeights);
                }
            }
        }
        preview_layer_attribution(&AttributionInputs {
            // In the closed-loop v1 Layer contract, a term is a distinct
            // internal purchase. This is NOT an external payment receipt.
            payment_id: self.term_id,
            term_id: self.term_id,
            gross_minor: self.gross_minor,
            currency: self.currency.clone(),
            policy_id: draft.policy_id.clone(),
            platform_fee_minor: draft.platform_fee_minor,
            weighting: draft.weighting,
            frozen_schedule_verified: true,
            allocation_basis_confirmed: draft.allocation_basis_confirmed,
            books,
        })
    }
}

#[derive(Clone)]
pub struct PgLayerProvenance {
    pool: PgPool,
}

impl PgLayerProvenance {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Verify the paid term and exact historical B-10 recipients from a
    /// repeatable, READ ONLY snapshot. No HTTP route uses this API.
    pub async fn load(
        &self,
        term_id: i64,
    ) -> Result<VerifiedLayerProvenance, LayerProvenanceError> {
        if term_id <= 0 {
            return Err(LayerProvenanceError::UnverifiedPurchase);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerProvenanceError::Unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|_| LayerProvenanceError::Unavailable)?;

        // The migration-enforced SQL proof joins the completed actor-scoped
        // command, exact offer, and unique immutable wallet journal debit.
        let term: Option<(i64, String)> = sqlx::query_as(
            "SELECT t.amount_minor, btrim(o.currency)::TEXT
             FROM commerce_layer_terms t
             JOIN commerce_layer_offers o
               ON o.id=t.offer_id AND o.layer_id=t.layer_id
             WHERE t.id=$1 AND d05_layer_term_has_wallet_evidence(t.id)",
        )
        .bind(term_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerProvenanceError::Unavailable)?;
        let (gross_minor, currency) = term.ok_or(LayerProvenanceError::UnverifiedPurchase)?;

        // Check the true origin..base paid composition, not the current
        // Layer version; later Book additions can change reader rights only.
        let expected_books: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT vb.book_id
             FROM commerce_layer_terms t
             JOIN commerce_layer_versions origin
               ON origin.id=t.access_origin_version_id AND origin.layer_id=t.layer_id
             JOIN commerce_layer_versions base
               ON base.id=t.base_version_id AND base.layer_id=t.layer_id
             JOIN commerce_layer_versions v ON v.layer_id=t.layer_id
               AND v.version_no BETWEEN origin.version_no AND base.version_no
             JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id
             WHERE t.id=$1 ORDER BY vb.book_id",
        )
        .bind(term_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| LayerProvenanceError::Unavailable)?;

        let rows = sqlx::query(
            "SELECT s.book_id,s.share_revision,l.user_id,l.share_percent,
                    a.share_percent AS authority_percent
             FROM commerce_layer_term_share_snapshots s
             LEFT JOIN commerce_layer_term_share_lines l
               ON l.term_id=s.term_id AND l.book_id=s.book_id
             LEFT JOIN revenue_share_assignments a
               ON a.book_id=s.book_id AND a.revision=s.share_revision
              AND a.user_id=l.user_id
             WHERE s.term_id=$1
             ORDER BY s.book_id,l.user_id",
        )
        .bind(term_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| LayerProvenanceError::Unavailable)?;

        // A matching member entry is not enough if another recipient from the
        // original revision is omitted. Require exact frozen line counts.
        let authority_counts: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT s.book_id,COUNT(a.user_id)::BIGINT
             FROM commerce_layer_term_share_snapshots s
             LEFT JOIN revenue_share_assignments a
               ON a.book_id=s.book_id AND a.revision=s.share_revision
             WHERE s.term_id=$1 GROUP BY s.book_id ORDER BY s.book_id",
        )
        .bind(term_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| LayerProvenanceError::Unavailable)?;

        let mut books: Vec<FrozenBook> = Vec::new();
        for row in rows {
            let book_id: i64 = row
                .try_get("book_id")
                .map_err(|_| LayerProvenanceError::Unavailable)?;
            let revision: i64 = row
                .try_get("share_revision")
                .map_err(|_| LayerProvenanceError::Unavailable)?;
            let user_id: Option<i64> = row
                .try_get("user_id")
                .map_err(|_| LayerProvenanceError::Unavailable)?;
            let percent: Option<i16> = row
                .try_get("share_percent")
                .map_err(|_| LayerProvenanceError::Unavailable)?;
            let authority: Option<i16> = row
                .try_get("authority_percent")
                .map_err(|_| LayerProvenanceError::Unavailable)?;
            let (user_id, percent) = match (user_id, percent) {
                (Some(user), Some(share)) if user > 0 && (1..=100).contains(&share) => {
                    (user, share)
                }
                _ => return Err(LayerProvenanceError::IncompleteFrozenSchedule),
            };
            if authority != Some(percent) || book_id <= 0 || revision <= 0 {
                return Err(LayerProvenanceError::IncompleteFrozenSchedule);
            }
            if books.last().map(|b| b.book_id) != Some(book_id) {
                books.push(FrozenBook {
                    book_id,
                    share_revision: revision,
                    weight_bps: None,
                    participants: Vec::new(),
                });
            }
            let book = books
                .last_mut()
                .ok_or(LayerProvenanceError::IncompleteFrozenSchedule)?;
            if book.share_revision != revision
                || book.participants.iter().any(|p| p.user_id == user_id)
            {
                return Err(LayerProvenanceError::IncompleteFrozenSchedule);
            }
            book.participants.push(FrozenParticipant {
                user_id,
                percent: percent as u16,
            });
        }

        let actual_books: Vec<i64> = books.iter().map(|b| b.book_id).collect();
        if expected_books.is_empty()
            || actual_books != expected_books
            || authority_counts.len() != books.len()
            || books.iter().zip(&authority_counts).any(|(book, count)| {
                count.0 != book.book_id
                    || count.1 != book.participants.len() as i64
                    || book
                        .participants
                        .iter()
                        .map(|p| u32::from(p.percent))
                        .sum::<u32>()
                        != 100
            })
        {
            return Err(LayerProvenanceError::IncompleteFrozenSchedule);
        }
        tx.commit()
            .await
            .map_err(|_| LayerProvenanceError::Unavailable)?;
        Ok(VerifiedLayerProvenance {
            term_id,
            gross_minor,
            currency,
            books,
        })
    }
}
