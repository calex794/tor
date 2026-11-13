use super::{limits, PgEditorial};
use crate::domain::editorial::*;
use async_trait::async_trait;
use sqlx::Row;
use tor_contracts::exposure::{
    CardObservation, EditorialObservationGuard, ObservationGuardError, PromotionSlot,
};

impl PgEditorial {
    /// Narrow report scope; the receiving report still applies current B access.
    /// Draft/paused/ended assignments are meaningful even before the first event.
    pub async fn assigned_book(&self, campaign_id: i64) -> Result<Option<i64>, EditorialError> {
        if campaign_id <= 0 {
            return Err(EditorialError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, true).await?;
        let book = sqlx::query_scalar("SELECT book_id FROM editorial_campaigns WHERE id=$1")
            .bind(campaign_id)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(book)
    }
    pub(super) async fn live_cards(
        &self,
        viewer: Option<&Viewer>,
        slot: PromotionSlot,
        host: Option<i64>,
    ) -> Result<Vec<EditorialCard>, EditorialError> {
        if viewer.is_some_and(|viewer| viewer.user_id <= 0)
            || host.is_some_and(|id| id <= 0)
            || (slot != PromotionSlot::HomeFeatured && host.is_none())
        {
            return Err(EditorialError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, true).await?;
        let rows=sqlx::query("WITH visible AS NOT MATERIALIZED (
          SELECT b.id,b.title FROM rec_materializable_books b
          WHERE ($1::bigint IS NULL OR EXISTS(SELECT 1 FROM users u JOIN sessions s ON s.user_id=u.id
              WHERE u.id=$1 AND u.account_state='active' AND s.token_hash=$4 AND s.revoked_at IS NULL AND s.expires_at>now()))
            AND (b.access_level='basic'
              OR (b.access_level='adult' AND EXISTS(SELECT 1 FROM users u WHERE u.id=$1 AND u.account_state='active' AND (u.adult_enabled OR EXISTS(SELECT 1 FROM user_level_grants g WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active))))
              OR (b.access_level='pro_adult' AND EXISTS(SELECT 1 FROM users u JOIN user_level_grants g ON g.user_id=u.id WHERE u.id=$1 AND u.account_state='active' AND g.level='pro_adult' AND g.is_active)))
            AND NOT EXISTS(SELECT 1 FROM rec_book_preferences p WHERE p.user_id=$1 AND p.book_id=b.id AND p.hidden AND p.expires_at>now())
        ), choices AS (
          SELECT c.id,c.revision,c.book_id,b.title,c.priority,
            row_number() OVER(PARTITION BY c.book_id ORDER BY c.priority DESC,c.id) AS duplicate_rank
          FROM editorial_campaigns c JOIN visible b ON b.id=c.book_id
          WHERE c.slot=$2 AND c.status='active' AND c.starts_at<=now() AND c.ends_at>now()
            AND ($3::bigint IS NULL OR (c.book_id<>$3 AND EXISTS(SELECT 1 FROM visible WHERE id=$3)))
            -- Only the free-chapter shelf has an editorial genre guard.
            -- With complete, explicitly disjoint genres and no shared tags,
            -- an administrator's priority cannot displace relevant organic books.
            -- Missing metadata is not proof of incompatibility, so fall back.
            AND (c.slot<>'chapter_end_free' OR NOT EXISTS (
              SELECT 1 FROM books host_book
              JOIN books promoted_book ON promoted_book.id=c.book_id
              WHERE host_book.id=$3
                AND COALESCE(cardinality(host_book.genres),0)>0
                AND COALESCE(cardinality(promoted_book.genres),0)>0
                AND NOT(host_book.genres && promoted_book.genres)
                AND NOT(COALESCE(host_book.tags,ARRAY[]::text[])
                        && COALESCE(promoted_book.tags,ARRAY[]::text[]))
            ))
        ) SELECT * FROM choices WHERE duplicate_rank=1 ORDER BY priority DESC,id LIMIT 4")
            .bind(viewer.map(|v|v.user_id)).bind(slot.as_str()).bind(host)
            .bind(viewer.map(|v|v.session_digest.as_slice())).fetch_all(&mut *tx).await?;
        let cards = rows
            .into_iter()
            .map(|row| {
                Ok(EditorialCard {
                    campaign_id: row.try_get("id")?,
                    revision: row.try_get("revision")?,
                    book_id: row.try_get("book_id")?,
                    title: row.try_get("title")?,
                    slot,
                })
            })
            .collect::<Result<Vec<_>, EditorialError>>()?;
        tx.commit().await?;
        Ok(cards)
    }

    /// The collector separately checks current viewer/Book access. This port
    /// only confirms that a genuine, still-current editorial assignment exists.
    pub async fn valid_observation(&self, card: &CardObservation) -> Result<bool, EditorialError> {
        let Some(placement) = card.editorial else {
            return Ok(false);
        };
        if !card.valid() {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, true).await?;
        let valid=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM editorial_campaigns c JOIN rec_materializable_books b ON b.id=c.book_id
          WHERE c.id=$1 AND c.revision=$2 AND c.book_id=$3 AND c.slot=$4
            AND c.status='active' AND c.starts_at<=clock_timestamp() AND c.ends_at>clock_timestamp())")
            .bind(placement.campaign_id).bind(placement.revision).bind(card.book_id).bind(placement.slot.as_str())
            .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(valid)
    }
}
#[async_trait]
impl EditorialObservationGuard for PgEditorial {
    async fn allowed(&self, card: &CardObservation) -> Result<bool, ObservationGuardError> {
        self.valid_observation(card)
            .await
            .map_err(|_| ObservationGuardError::Unavailable)
    }
    async fn campaign_book(&self, campaign_id: i64) -> Result<Option<i64>, ObservationGuardError> {
        self.assigned_book(campaign_id)
            .await
            .map_err(|_| ObservationGuardError::Unavailable)
    }
}
