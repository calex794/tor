use super::*;
use crate::domain::feedback::{
    observation, token_digest, ImpressionClaim, Observation, QualityRow, MAX_ITEMS,
};
use crate::services::feedback::validate_actor;
use sqlx::Row;

impl PgRecommendations {
    pub async fn impressions(
        &self,
        actor: i64,
        session: &[u8; 32],
        claims: &[ImpressionClaim],
    ) -> Result<(), FeedbackError> {
        validate_actor(actor)?;
        if claims.is_empty() || claims.len() > MAX_ITEMS {
            return Err(FeedbackError::Invalid);
        }
        for claim in claims {
            claim.validate()?;
        }
        let mut tx = self.pool.begin().await?;
        let subject = lock_subject(&mut tx, actor).await?;
        for claim in claims.iter().filter(|c| c.qualified()) {
            let digest = token_digest(&claim.ticket)?;
            let id: Option<String>=sqlx::query_scalar(
                "UPDATE rec_assignments a SET viewed_at=COALESCE(a.viewed_at,now())
                 WHERE a.ticket_digest=$1 AND a.subject_id=$2::uuid AND a.session_digest=$3 AND a.expires_at>now()
                   AND EXISTS(SELECT 1 FROM rec_public_books WHERE id=a.book_id)
                 RETURNING a.id::text")
                .bind(digest.as_slice()).bind(&subject).bind(session.as_slice()).fetch_optional(&mut *tx).await?;
            let id = id.ok_or(FeedbackError::NotFound)?;
            sqlx::query("INSERT INTO rec_attribution_events(assignment_id,kind,occurred_at,trust) SELECT id,'impression',viewed_at,'client_observed' FROM rec_assignments WHERE id=$1::uuid ON CONFLICT(assignment_id,kind) DO NOTHING")
                .bind(&id).execute(&mut *tx).await?;
            dirty(&mut tx, &id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Caller is an authenticated operator. Return bounded aggregate facts, not raw identities.
    pub async fn quality(&self, days: i32) -> Result<Vec<QualityRow>, FeedbackError> {
        if !(1..=30).contains(&days) {
            return Err(FeedbackError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT bucket_date::text,q.book_id::text,surface,source,reason,generation,algorithm_version,position,
                    impressions,paired_clicks,unpaired_clicks,starts,completions
             FROM rec_quality_daily q JOIN rec_public_books b ON b.id=q.book_id
             WHERE bucket_date>=(now() AT TIME ZONE 'UTC')::date-$1
             ORDER BY bucket_date DESC,q.book_id,surface,source,reason,generation,algorithm_version,position LIMIT 100",
        )
        .bind(days)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(QualityRow {
                    day: r.try_get("bucket_date")?,
                    book_id: r.try_get("book_id")?,
                    surface: r.try_get("surface")?,
                    source: r.try_get("source")?,
                    reason: r.try_get("reason")?,
                    generation: r.try_get("generation")?,
                    algorithm_version: r.try_get("algorithm_version")?,
                    position: r.try_get("position")?,
                    impressions: r.try_get("impressions")?,
                    paired_clicks: r.try_get("paired_clicks")?,
                    unpaired_clicks: r.try_get("unpaired_clicks")?,
                    starts: r.try_get("starts")?,
                    completions: r.try_get("completions")?,
                })
            })
            .collect()
    }

    /// Diagnostic [pending, positive, censored, no-verified-start], never a ranking penalty.
    pub async fn own_observations(&self, actor: i64) -> Result<[u64; 4], FeedbackError> {
        validate_actor(actor)?;
        let rows=sqlx::query(
            "SELECT extract(epoch FROM now())::bigint AS at,
               extract(epoch FROM a.book_opened_at)::bigint AS opened,
               a.progress_observed_at IS NOT NULL AS progressed,
               EXISTS(SELECT 1 FROM rec_free_readable_books WHERE book_id=a.book_id) AS readable,
               EXISTS(SELECT 1 FROM rec_worker_health WHERE last_success>now()-interval '30 seconds') AS healthy
             FROM rec_assignments a JOIN rec_subjects s ON s.subject_id=a.subject_id
             WHERE s.user_id=$1 AND s.enabled AND a.created_at>now()-interval '14 days'
             ORDER BY a.created_at DESC LIMIT 1000")
            .bind(actor).fetch_all(&self.pool).await?;
        let mut counts = [0; 4];
        for r in rows {
            let result = observation(
                r.try_get("at")?,
                r.try_get("opened")?,
                r.try_get("progressed")?,
                r.try_get("readable")?,
                r.try_get("healthy")?,
            );
            let index = match result {
                Observation::Pending => 0,
                Observation::Positive => 1,
                Observation::Censored => 2,
                Observation::NoVerifiedStart => 3,
            };
            counts[index] += 1;
        }
        Ok(counts)
    }
}
