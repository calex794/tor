//! Only already approved and currently public/basic Book-linked surprises.
//! No private chapter text or future-page metadata is returned.
use super::{PgSurprises, SurpriseContent, SurpriseError};
use sqlx::Row;

impl PgSurprises {
    pub async fn open(
        &self,
        user_id: i64,
        utc_day: i64,
    ) -> Result<Option<SurpriseContent>, SurpriseError> {
        if user_id <= 0 || utc_day <= 18000 {
            return Err(SurpriseError::Invalid);
        }
        let row = sqlx::query(
            "SELECT d.content_ticket FROM surprise_daily_decisions d
             JOIN users u ON u.id=d.user_id AND u.account_state='active'
             LEFT JOIN surprise_user_preferences p ON p.user_id=d.user_id
             JOIN surprise_runtime_policy policy ON policy.singleton
             WHERE d.user_id=$1 AND d.utc_day=$2
               AND d.state IN ('claimed','opened')
               AND d.claimed_at>now()-interval '30 minutes'
               AND COALESCE(p.disabled,false)=false AND policy.enabled",
        )
        .bind(user_id)
        .bind(utc_day)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let ticket: i64 = row.try_get("content_ticket")?;
        // Fail closed on elevated age tiers in V1. Later adult surfaces need
        // independently tested actor-specific entitlement checks.
        let row = sqlx::query(
            "SELECT c.id,c.kind,c.title,c.body,c.book_id,c.editorial
             FROM surprise_content c
             LEFT JOIN books b ON b.id=c.book_id
             LEFT JOIN book_recommendation_audiences a ON a.book_id=c.book_id
             WHERE c.state='approved' AND c.starts_at<=now()
               AND (c.ends_at IS NULL OR c.ends_at>now())
               AND (c.book_id IS NULL OR (
                   b.visibility='public' AND b.visibility_status='active'
                   AND b.lifecycle_status<>'pending_deletion'
                   AND NOT b.admin_hold AND NOT b.is_deleted
                   AND b.access_level='basic'
                   AND COALESCE(a.audience,'general')='general'
               ))
             ORDER BY (c.id # $1::bigint) DESC,c.id DESC LIMIT 1",
        )
        .bind(ticket)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let result = SurpriseContent {
            id: row.try_get("id")?,
            kind: row.try_get("kind")?,
            title: row.try_get("title")?,
            body: row.try_get("body")?,
            book_id: row.try_get("book_id")?,
            editorial: row.try_get("editorial")?,
        };
        // An opened receipt is idempotent; it cannot pay or increase chances.
        sqlx::query(
            "UPDATE surprise_daily_decisions SET state='opened',opened_at=COALESCE(opened_at,now())
             WHERE user_id=$1 AND utc_day=$2 AND state IN ('claimed','opened')",
        )
        .bind(user_id)
        .bind(utc_day)
        .execute(&self.pool)
        .await?;
        Ok(Some(result))
    }
}
