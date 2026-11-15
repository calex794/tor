use super::{binding, unavailable, PgFlowFunnel, CONSUMER};
use crate::services::{flow_funnel::FlowFunnelError, visits::valid_uuid};
use sqlx::{Postgres, Row, Transaction};

pub(super) struct ReadEvidence {
    pub id: String,
    pub actor: i64,
    pub book: i64,
    pub chapter: i64,
    pub at: String,
    pub kind: i16,
    pub first: bool,
}

pub(super) struct Scope {
    pub subject: String,
    pub epoch: String,
    pub enabled_at: String,
}

impl PgFlowFunnel {
    pub(super) async fn pending_reads(&self) -> Result<Vec<String>, FlowFunnelError> {
        let mut tx = self.begin().await?;
        let result = sqlx::query_scalar(
            "SELECT v.event_id::text FROM reader_flow_verified_read_events v
             JOIN reader_flow_verified_read_receipts r ON r.event_id=v.event_id AND r.occurred_at=v.occurred_at
             JOIN rec_subjects s ON s.subject_id=r.subject_id AND s.flow_epoch=r.flow_epoch
               AND s.user_id=v.user_id AND s.enabled
             JOIN users u ON u.id=s.user_id AND u.account_state='active'
             WHERE v.occurred_at>clock_timestamp()-interval '30 days'
               AND v.occurred_at<=clock_timestamp()-interval '305 seconds'
               AND v.occurred_at>=s.flow_enabled_at
               AND NOT EXISTS(SELECT 1 FROM event_deliveries d WHERE d.event_id=v.event_id AND d.consumer=$1)
             ORDER BY v.occurred_at,v.kind,v.event_id LIMIT 64",
        )
        .bind(CONSUMER)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }

    /// G may replay a canonical event. Browser claims and a naked outbox row
    /// cannot pass C's historical proof view. False is replay or deferred work.
    pub async fn accept_read_event(
        &self,
        event: &str,
        collect: bool,
    ) -> Result<bool, FlowFunnelError> {
        if !valid_uuid(event) {
            return Err(FlowFunnelError::Invalid);
        }
        if !collect {
            return Ok(false);
        }
        let mut tx = self.begin().await?;
        let evidence = evidence(&mut tx, event)
            .await?
            .ok_or(FlowFunnelError::Invalid)?;
        let mature: bool = sqlx::query_scalar(
            "SELECT $1::timestamptz<=clock_timestamp()-interval '305 seconds'
               AND $1::timestamptz>clock_timestamp()-interval '30 days'",
        )
        .bind(&evidence.at)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if !mature {
            return Ok(false);
        }
        let start = if evidence.kind == 1 {
            let id: Option<String> = sqlx::query_scalar(
                "SELECT v.event_id::text FROM reader_flow_verified_read_events v
                 JOIN reader_flow_verified_read_receipts r ON r.event_id=v.event_id AND r.occurred_at=v.occurred_at
                 WHERE v.user_id=$1 AND v.book_id=$2 AND v.chapter_id=$3 AND v.kind=0
                   AND v.occurred_at<=$4::timestamptz
                 ORDER BY v.occurred_at,v.event_id LIMIT 1",
            )
            .bind(evidence.actor)
            .bind(evidence.book)
            .bind(evidence.chapter)
            .bind(&evidence.at)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            match id {
                Some(id) => self::evidence(&mut tx, &id).await?,
                None => None,
            }
        } else {
            None
        };
        // Existing REC consumers lock outbox before the subject. Keep that
        // order, locking completion+its genuine start in UUID order together.
        let mut ids = vec![evidence.id.clone()];
        if let Some(start) = &start {
            ids.push(start.id.clone());
        }
        sqlx::query("SELECT id FROM outbox_events WHERE id IN(SELECT value::uuid FROM unnest($1::text[]) value) ORDER BY id FOR UPDATE")
            .bind(&ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        let replay: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM event_deliveries WHERE consumer=$1 AND event_id=$2::uuid)",
        )
        .bind(CONSUMER)
        .bind(event)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if replay {
            return Ok(false);
        }
        // Opt-out can commit after the first evidence read. A missing current
        // proof must not acquire a new FLOW delivery after waiting for locks.
        let Some(scope) = scope(&mut tx, &evidence).await? else {
            return Ok(false);
        };
        let pinned = pin_proofs(&mut tx, &scope, &ids).await?;
        if !pinned.contains(&evidence.id) {
            return Ok(false);
        }
        let source = start.as_ref().unwrap_or(&evidence);
        if source.kind == 0 {
            if binding::waiting_for_books(&mut tx, &scope, source).await? {
                return Ok(false);
            }
            let cohort = binding::apply_start(&mut tx, &scope, source).await?;
            if let (Some(cohort), Some(start)) = (cohort, start.as_ref()) {
                sqlx::query(
                            "UPDATE flow_book_cohorts SET completed=true,
                               first_completion_event_id=COALESCE(first_completion_event_id,$2::uuid)
                             WHERE id=$1::uuid AND $3::timestamptz<=$4::timestamptz+interval '14 days'
                               AND $3::timestamptz<expires_at AND finalized_at IS NULL",
                        )
                        .bind(cohort)
                        .bind(&evidence.id)
                        .bind(&evidence.at)
                        .bind(&start.at)
                        .execute(&mut *tx)
                        .await
                        .map_err(unavailable)?;
                record_delivery(&mut tx, &scope, &start.id).await?;
            }
        }
        record_delivery(&mut tx, &scope, event).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(true)
    }
}

async fn evidence(
    tx: &mut Transaction<'_, Postgres>,
    event: &str,
) -> Result<Option<ReadEvidence>, FlowFunnelError> {
    let row = sqlx::query(
        "SELECT v.event_id::text,v.user_id,v.book_id,v.chapter_id,v.occurred_at::text,v.kind,v.first_for_book
         FROM reader_flow_verified_read_events v
         WHERE v.event_id=$1::uuid AND v.occurred_at<=clock_timestamp()",
    )
    .bind(event)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
    row.map(|row| {
        Ok(ReadEvidence {
            id: row.try_get("event_id").map_err(unavailable)?,
            actor: row.try_get("user_id").map_err(unavailable)?,
            book: row.try_get("book_id").map_err(unavailable)?,
            chapter: row.try_get("chapter_id").map_err(unavailable)?,
            at: row.try_get("occurred_at").map_err(unavailable)?,
            kind: row.try_get("kind").map_err(unavailable)?,
            first: row.try_get("first_for_book").map_err(unavailable)?,
        })
    })
    .transpose()
}

async fn scope(
    tx: &mut Transaction<'_, Postgres>,
    event: &ReadEvidence,
) -> Result<Option<Scope>, FlowFunnelError> {
    let row = sqlx::query(
        "SELECT s.subject_id::text,s.flow_epoch::text,s.flow_enabled_at::text
         FROM rec_subjects s JOIN users u ON u.id=s.user_id AND u.account_state='active'
         WHERE s.user_id=$1 AND s.enabled AND s.flow_epoch IS NOT NULL
           AND s.flow_enabled_at<=$2::timestamptz
         FOR UPDATE OF s FOR SHARE OF u",
    )
    .bind(event.actor)
    .bind(&event.at)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
    let Some(row) = row else { return Ok(None) };
    let scope = Scope {
        subject: row.try_get("subject_id").map_err(unavailable)?,
        epoch: row.try_get("flow_epoch").map_err(unavailable)?,
        enabled_at: row.try_get("flow_enabled_at").map_err(unavailable)?,
    };
    Ok(Some(scope))
}

async fn pin_proofs(
    tx: &mut Transaction<'_, Postgres>,
    scope: &Scope,
    ids: &[String],
) -> Result<Vec<String>, FlowFunnelError> {
    // Pin both the completion and its genuine start in one stable order after
    // taking the subject lock. C's expiry DELETE then cannot race the receipts.
    sqlx::query_scalar(
        "SELECT r.event_id::text FROM reader_flow_verified_read_receipts r
         JOIN reader_flow_verified_read_events v ON v.event_id=r.event_id AND v.occurred_at=r.occurred_at
         WHERE r.event_id IN(SELECT value::uuid FROM unnest($1::text[]) value)
           AND r.subject_id=$2::uuid AND r.flow_epoch=$3::uuid
           AND r.occurred_at>clock_timestamp()-interval '30 days'
         ORDER BY r.event_id FOR KEY SHARE OF r",
    ).bind(ids).bind(&scope.subject).bind(&scope.epoch)
        .fetch_all(&mut **tx).await.map_err(unavailable)
}

pub(super) async fn lock_proof(
    tx: &mut Transaction<'_, Postgres>,
    scope: &Scope,
    event: &ReadEvidence,
) -> Result<bool, FlowFunnelError> {
    // Subject first, receipt second. C expiry waits on this row, then its
    // DELETE trigger removes only our delivery after this transaction commits.
    let proof: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM reader_flow_verified_read_receipts r
         WHERE r.event_id=$1::uuid AND r.subject_id=$2::uuid AND r.flow_epoch=$3::uuid
           AND r.occurred_at=$4::timestamptz AND r.occurred_at>clock_timestamp()-interval '30 days'
         FOR KEY SHARE OF r",
    )
    .bind(&event.id)
    .bind(&scope.subject)
    .bind(&scope.epoch)
    .bind(&event.at)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(proof.is_some())
}

async fn record_delivery(
    tx: &mut Transaction<'_, Postgres>,
    scope: &Scope,
    event: &str,
) -> Result<(), FlowFunnelError> {
    sqlx::query("INSERT INTO event_deliveries(consumer,event_id)
        SELECT $1,event_id FROM reader_flow_verified_read_receipts
        WHERE event_id=$2::uuid AND subject_id=$3::uuid AND flow_epoch=$4::uuid ON CONFLICT DO NOTHING")
        .bind(CONSUMER).bind(event).bind(&scope.subject).bind(&scope.epoch)
        .execute(&mut **tx).await.map_err(unavailable)?;
    Ok(())
}
