use super::{unavailable, PgFlowFunnel};
use crate::services::flow_funnel::FlowFunnelError;
use sqlx::Row;

impl PgFlowFunnel {
    pub(super) async fn capture_actions(&self) -> Result<u64, FlowFunnelError> {
        let mut tx = self.begin().await?;
        let events: Vec<String> = sqlx::query_scalar(
            "SELECT a.event_id::text FROM reader_flow_verified_book_actions a
             WHERE a.occurred_at<=clock_timestamp()-interval '305 seconds'
               AND a.occurred_at>clock_timestamp()-interval '30 days'
               AND NOT EXISTS(SELECT 1 FROM flow_book_action_receipts r WHERE r.event_id=a.event_id)
             ORDER BY a.occurred_at,a.event_id LIMIT 64",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        let mut affected = 0;
        for event in events {
            let mut tx = self.begin().await?;
            let row = sqlx::query(
                "SELECT a.subject_id::text,a.flow_epoch::text,a.book_id,a.kind,a.occurred_at::text
                 FROM reader_flow_verified_book_actions a JOIN rec_subjects s
                   ON s.subject_id=a.subject_id AND s.flow_epoch=a.flow_epoch AND s.enabled
                 WHERE a.event_id=$1::uuid AND a.occurred_at>clock_timestamp()-interval '30 days' FOR UPDATE OF s",
            )
            .bind(&event)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            let Some(row) = row else { continue };
            let subject: String = row.try_get("subject_id").map_err(unavailable)?;
            let epoch: String = row.try_get("flow_epoch").map_err(unavailable)?;
            let book: i64 = row.try_get("book_id").map_err(unavailable)?;
            let kind: i16 = row.try_get("kind").map_err(unavailable)?;
            let at: String = row.try_get("occurred_at").map_err(unavailable)?;
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM flow_visit_receipts WHERE subject_id=$1::uuid AND flow_epoch=$2::uuid
                   AND book_id=$3 AND NOT funnel_processed AND observed_at<=$4::timestamptz
                   AND observed_at>=$4::timestamptz-interval '7 days')",
            ).bind(&subject).bind(&epoch).bind(book).bind(&at).fetch_one(&mut *tx).await.map_err(unavailable)?;
            if waiting {
                continue;
            }
            let inserted = sqlx::query(
                "INSERT INTO flow_book_action_receipts(event_id,subject_id,flow_epoch,occurred_at)
                 VALUES($1::uuid,$2::uuid,$3::uuid,$4::timestamptz) ON CONFLICT DO NOTHING",
            )
            .bind(&event)
            .bind(&subject)
            .bind(&epoch)
            .bind(&at)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            if inserted.rows_affected() == 1 {
                sqlx::query(
                    "UPDATE flow_book_cohorts SET
                       saved=saved OR $2=1,followed=followed OR $2=2,
                       saved_event_id=CASE WHEN $2=1 THEN COALESCE(saved_event_id,$1::uuid) ELSE saved_event_id END,
                       followed_event_id=CASE WHEN $2=2 THEN COALESCE(followed_event_id,$1::uuid) ELSE followed_event_id END
                     WHERE id=(SELECT c.id FROM flow_book_cohorts c
                       WHERE c.subject_id=$3::uuid AND c.flow_epoch=$4::uuid AND c.book_id=$5
                         AND c.first_entry_at<=$6::timestamptz AND c.first_entry_at>=$6::timestamptz-interval '7 days'
                         AND c.expires_at>clock_timestamp() AND c.expires_at>$6::timestamptz AND c.finalized_at IS NULL
                       ORDER BY c.first_entry_at DESC,c.id LIMIT 1)",
                ).bind(&event).bind(kind).bind(&subject).bind(&epoch).bind(book).bind(&at)
                    .execute(&mut *tx).await.map_err(unavailable)?;
            }
            affected += inserted.rows_affected();
            tx.commit().await.map_err(unavailable)?;
        }
        Ok(affected)
    }
}
