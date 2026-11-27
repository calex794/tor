//! Explicit, conservative billing reconciliation. Never activates sending.
use super::{lock_guard, operator, revision, CostAdminError};
use crate::domain::cost::MonthUsage;
use crate::repositories::cost::{load, PgMailCost};
use sqlx::{Postgres, Transaction};

pub struct MonthReconciliation {
    pub usage: MonthUsage,
    /// SHA-256 of independently reviewed billing/recovery evidence. The
    /// caller must verify that evidence; the hash alone proves nothing.
    pub evidence_sha256: [u8; 32],
}

impl PgMailCost {
    /// Initialize only the current Moscow billing month, or tighten an
    /// existing month's estimate. This is a stopped-only operator action.
    /// No counter reset, refund, historical edit or implicit activation.
    pub async fn admin_reconcile_month(
        &self,
        digest: &[u8; 32],
        expected_revision: i64,
        request: &MonthReconciliation,
    ) -> Result<(), CostAdminError> {
        use CostAdminError::*;
        let next = &request.usage;
        if expected_revision <= 0
            || request.evidence_sha256 == [0; 32]
            || next.estimate_kopecks(1).is_none()
        {
            return Err(Invalid);
        }

        let mut tx = self.pool.begin().await?;
        let actor = operator(&mut tx, digest).await?;
        lock_guard(&mut tx).await?;
        if revision(&mut tx).await? != expected_revision {
            return Err(Conflict);
        }
        let current: String = sqlx::query_scalar(
            "SELECT date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date::text",
        )
        .fetch_one(&mut *tx)
        .await?;
        if next.month_start != current {
            return Err(Conflict);
        }
        let snapshot = load(&mut tx).await?.ok_or(Unavailable)?;
        if snapshot.limits.enabled || snapshot.boundary_pause {
            return Err(Conflict);
        }
        let old = snapshot.usage;
        if let Some(previous) = &old {
            if previous.month_start != current || previous.estimate_kopecks(0).is_none() {
                return Err(Unavailable);
            }
            if next.free_allowance > previous.free_allowance
                || next.baseline_kopecks < previous.baseline_kopecks
                || next.rate_per_thousand < previous.rate_per_thousand
                || next.reserved_attempts < previous.reserved_attempts
            {
                return Err(Invalid);
            }
        }
        // Short-lived reservation rows and longer-lived feedback correlations
        // overlap. Count each known attempt once, conservatively including
        // future-dated rows. External evidence is still required for lost or
        // pruned history; these rows are only a lower bound.
        let known: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ( \
             SELECT outbox_id,generation FROM mail_send_reservations \
             WHERE reserved_at>=($1::date::timestamp AT TIME ZONE 'Europe/Moscow') \
             UNION SELECT outbox_id,generation FROM mail_feedback_attempts \
             WHERE reserved_at>=($1::date::timestamp AT TIME ZONE 'Europe/Moscow') \
             ) known_attempts",
        )
        .bind(&current)
        .fetch_one(&mut *tx)
        .await?;
        if next.reserved_attempts < known {
            return Err(Invalid);
        }

        write_month(&mut tx, next, old.as_ref()).await?;
        let new_revision = expected_revision.checked_add(1).ok_or(Unavailable)?;
        let changed = sqlx::query(
            "UPDATE mail_cost_policy SET revision=$1 WHERE singleton AND NOT enabled \
             AND revision=$2 AND $3::date= \
               date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date \
             AND clock_timestamp() AT TIME ZONE 'Europe/Moscow' < \
               $3::date+interval '1 month'-interval '5 minutes'",
        )
        .bind(new_revision)
        .bind(expected_revision)
        .bind(&current)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(Conflict);
        }

        sqlx::query(
            "INSERT INTO mail_cost_month_reconciliation_audit \
             (actor_user_id,month_start,old_revision,new_revision,evidence_sha256, \
              old_free_allowance,old_baseline_kopecks,old_rate_per_thousand,old_reserved_attempts, \
              new_free_allowance,new_baseline_kopecks,new_rate_per_thousand,new_reserved_attempts) \
             VALUES($1,$2::date,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(actor)
        .bind(&current)
        .bind(expected_revision)
        .bind(new_revision)
        .bind(request.evidence_sha256.as_slice())
        .bind(old.as_ref().map(|value| value.free_allowance))
        .bind(old.as_ref().map(|value| value.baseline_kopecks))
        .bind(old.as_ref().map(|value| value.rate_per_thousand))
        .bind(old.as_ref().map(|value| value.reserved_attempts))
        .bind(next.free_allowance)
        .bind(next.baseline_kopecks)
        .bind(next.rate_per_thousand)
        .bind(next.reserved_attempts)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn write_month(
    tx: &mut Transaction<'_, Postgres>,
    next: &MonthUsage,
    old: Option<&MonthUsage>,
) -> Result<(), CostAdminError> {
    let count = if let Some(old) = old {
        sqlx::query(
            "UPDATE mail_cost_months SET free_allowance=$2,baseline_kopecks=$3, \
             rate_per_thousand=$4,reserved_attempts=$5 WHERE month_start=$1::date \
             AND free_allowance=$6 AND baseline_kopecks=$7 \
             AND rate_per_thousand=$8 AND reserved_attempts=$9",
        )
        .bind(&next.month_start)
        .bind(next.free_allowance)
        .bind(next.baseline_kopecks)
        .bind(next.rate_per_thousand)
        .bind(next.reserved_attempts)
        .bind(old.free_allowance)
        .bind(old.baseline_kopecks)
        .bind(old.rate_per_thousand)
        .bind(old.reserved_attempts)
        .execute(&mut **tx)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO mail_cost_months \
             (month_start,free_allowance,baseline_kopecks,rate_per_thousand,reserved_attempts) \
             VALUES($1::date,$2,$3,$4,$5) ON CONFLICT(month_start) DO NOTHING",
        )
        .bind(&next.month_start)
        .bind(next.free_allowance)
        .bind(next.baseline_kopecks)
        .bind(next.rate_per_thousand)
        .bind(next.reserved_attempts)
        .execute(&mut **tx)
        .await?
        .rows_affected()
    };
    if count != 1 {
        return Err(CostAdminError::Conflict);
    }
    Ok(())
}
