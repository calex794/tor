//! MOD-OPS-ALERT-01 / #661: durable internal *observations*, not delivery.
//! No scheduler calls this function and no route exposes these records.
//! Thresholds are diagnostic buckets, not an approved business SLA.
use super::PgAccounts;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverdueScanResult {
    pub opened: u64,
    pub resolved: u64,
}

impl PgAccounts {
    /// Called only when the operator opted in to background observation.
    /// Fail startup rather than silently operating without the additive ledger.
    /// The restricted tor_app role needs read access, not schema-owner DDL.
    pub async fn probe_moderation_overdue_observations(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT id,signal_state FROM moderation_overdue_signals LIMIT 0")
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Manually invoked internal scan; no user-supplied values are used in SQL.
    /// Serializes concurrent scanners and reconciles immutable request keys.
    ///
    /// Source revision 0 means the *initial* Book registration or a unique
    /// cover request ID. Ordinary deletion uses the exact lifecycle revision.
    /// A later registration reapplication will require its own request key.
    /// This only creates/cancels internal records. No email, push, outbox
    /// delivery, UI alert, or external side effect occurs.
    pub async fn scan_moderation_overdue_observations(
        &self,
        batch_limit: u16,
    ) -> Result<OverdueScanResult, sqlx::Error> {
        if !(1..=500).contains(&batch_limit) {
            return Err(sqlx::Error::Protocol(
                "moderation observation batch must be within 1..=500".into(),
            ));
        }
        let mut tx = self.pool().begin().await?;
        // Two independent worker instances cannot interleave observation
        // inserts with cancellation. This does not lock moderator decisions;
        // any racing resolved request is reconciled on the next scan.
        sqlx::query("SELECT pg_advisory_xact_lock(661, 2026)")
            .fetch_one(&mut *tx)
            .await?;
        let opened = sqlx::query(
            "WITH pending(request_kind,source_id,source_revision,submitted_at) AS (
                SELECT 'registration'::text,b.id,0::bigint,b.created_at
                FROM books b
                WHERE b.registration_requires_review
                  AND b.visibility_status='pending_moderation'
                  AND b.registration_approval_decision_id IS NULL
                  AND NOT b.is_deleted
                  AND NOT EXISTS (
                     SELECT 1 FROM book_registration_decisions d WHERE d.book_id=b.id
                  )
                UNION ALL
                SELECT 'cover'::text,r.id,0::bigint,r.submitted_at
                FROM book_cover_review_requests r
                JOIN books b ON b.id=r.book_id
                WHERE r.state='pending' AND NOT b.is_deleted
                UNION ALL
                SELECT 'deletion'::text,r.book_id,r.request_revision,r.requested_at
                FROM book_ordinary_deletion_requests r
                JOIN books b ON b.id=r.book_id
                WHERE b.lifecycle_status='pending_deletion'
                  AND b.lifecycle_revision=r.request_revision
                  AND NOT b.is_deleted
                  AND NOT EXISTS (
                     SELECT 1 FROM book_ordinary_deletion_decisions d
                     WHERE d.book_id=r.book_id AND d.request_revision=r.request_revision
                  )
            ),
            due AS (
                SELECT p.request_kind,p.source_id,p.source_revision,t.hours
                FROM pending p
                CROSS JOIN (VALUES (24::smallint),(72::smallint)) t(hours)
                LEFT JOIN moderation_overdue_signals existing
                  ON existing.request_kind=p.request_kind
                 AND existing.source_id=p.source_id
                 AND existing.source_revision=p.source_revision
                 AND existing.threshold_hours=t.hours
                WHERE p.submitted_at <= now()-make_interval(hours=>t.hours::integer)
                  AND existing.id IS NULL
                ORDER BY p.submitted_at,p.request_kind,p.source_id,t.hours
                LIMIT $1
            )
            INSERT INTO moderation_overdue_signals
                (request_kind,source_id,source_revision,threshold_hours)
            SELECT request_kind,source_id,source_revision,hours FROM due
            ON CONFLICT (request_kind,source_id,source_revision,threshold_hours)
            DO NOTHING",
        )
        .bind(i64::from(batch_limit))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // Limit cancellation as well as insertion: a backlog must not turn
        // a 60-second worker tick into an unbounded database transaction.
        // A later tick continues from remaining open records.
        let resolved = sqlx::query(
            "WITH stale AS (
               SELECT s.id FROM moderation_overdue_signals s
               WHERE s.signal_state='open'
                 AND NOT (
                 (s.request_kind='registration' AND s.source_revision=0
                  AND EXISTS (
                    SELECT 1 FROM books b WHERE b.id=s.source_id
                      AND b.registration_requires_review
                      AND b.visibility_status='pending_moderation'
                      AND b.registration_approval_decision_id IS NULL
                      AND NOT b.is_deleted
                      AND b.created_at <= now()-make_interval(hours=>s.threshold_hours::integer)
                      AND NOT EXISTS (
                        SELECT 1 FROM book_registration_decisions d WHERE d.book_id=b.id
                      )
                  ))
                 OR
                 (s.request_kind='cover' AND s.source_revision=0
                  AND EXISTS (
                    SELECT 1 FROM book_cover_review_requests r
                    JOIN books b ON b.id=r.book_id
                    WHERE r.id=s.source_id AND r.state='pending'
                      AND NOT b.is_deleted
                      AND r.submitted_at <= now()-make_interval(hours=>s.threshold_hours::integer)
                  ))
                 OR
                 (s.request_kind='deletion' AND EXISTS (
                    SELECT 1 FROM book_ordinary_deletion_requests r
                    JOIN books b ON b.id=r.book_id
                    WHERE r.book_id=s.source_id
                      AND r.request_revision=s.source_revision
                      AND b.lifecycle_status='pending_deletion'
                      AND b.lifecycle_revision=r.request_revision
                      AND NOT b.is_deleted
                      AND r.requested_at <= now()-make_interval(hours=>s.threshold_hours::integer)
                      AND NOT EXISTS (
                        SELECT 1 FROM book_ordinary_deletion_decisions d
                        WHERE d.book_id=r.book_id
                          AND d.request_revision=r.request_revision
                      )
                 ))
               )
               ORDER BY s.id
               LIMIT $1
             )
             UPDATE moderation_overdue_signals s
             SET signal_state='resolved',resolved_at=clock_timestamp()
             FROM stale
             WHERE s.id=stale.id AND s.signal_state='open'",
        )
        .bind(i64::from(batch_limit))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(OverdueScanResult { opened, resolved })
    }
}
