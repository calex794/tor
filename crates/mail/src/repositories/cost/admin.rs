//! Operator-only cost controls. No activation, month reset or provider I/O.
use super::{load, PgMailCost};
use crate::domain::cost::{CostLimits, CostSnapshot};
use sqlx::{Postgres, Transaction};

const MAX_LIMIT_KOPECKS: i64 = 10_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LimitChange {
    pub warning_1: i64,
    pub warning_2: i64,
    pub news_pause: i64,
    pub monthly_limit: i64,
}

impl LimitChange {
    pub fn valid(self) -> bool {
        0 < self.warning_1
            && self.warning_1 <= self.warning_2
            && self.warning_2 <= self.news_pause
            && self.news_pause < self.monthly_limit
            && self.monthly_limit <= MAX_LIMIT_KOPECKS
            && [
                self.warning_1,
                self.warning_2,
                self.news_pause,
                self.monthly_limit,
            ]
            .iter()
            .all(|value| value % 100 == 0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CostAdminError {
    Invalid,
    Forbidden,
    Conflict,
    Unavailable,
}

impl From<sqlx::Error> for CostAdminError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

pub struct CostAdminView {
    pub revision: i64,
    pub snapshot: CostSnapshot,
}

impl PgMailCost {
    pub async fn admin_view(&self, digest: &[u8; 32]) -> Result<CostAdminView, CostAdminError> {
        let mut tx = self.pool.begin().await?;
        let _actor = operator(&mut tx, digest).await?;
        lock_guard(&mut tx).await?;
        let revision = revision(&mut tx).await?;
        let snapshot = load(&mut tx).await?.ok_or(CostAdminError::Unavailable)?;
        tx.commit().await?;
        Ok(CostAdminView { revision, snapshot })
    }

    pub async fn admin_save_limits(
        &self,
        digest: &[u8; 32],
        expected_revision: i64,
        limits: LimitChange,
    ) -> Result<(), CostAdminError> {
        if expected_revision <= 0 || !limits.valid() {
            return Err(CostAdminError::Invalid);
        }
        self.admin_change(digest, Some((expected_revision, limits)))
            .await
    }

    /// Stop is intentionally independent of the browser's displayed revision.
    /// Repeating it is idempotent; no form in this module can enable sending.
    pub async fn admin_stop(&self, digest: &[u8; 32]) -> Result<(), CostAdminError> {
        self.admin_change(digest, None).await
    }

    async fn admin_change(
        &self,
        digest: &[u8; 32],
        requested: Option<(i64, LimitChange)>,
    ) -> Result<(), CostAdminError> {
        let mut tx = self.pool.begin().await?;
        let actor = operator(&mut tx, digest).await?;
        lock_guard(&mut tx).await?;
        let old_revision = revision(&mut tx).await?;
        let old = load(&mut tx)
            .await?
            .ok_or(CostAdminError::Unavailable)?
            .limits;
        let (action, next) = match requested {
            Some((expected, limits)) if expected == old_revision => (
                "limits",
                CostLimits {
                    enabled: old.enabled,
                    warning_1: limits.warning_1,
                    warning_2: limits.warning_2,
                    news_pause: limits.news_pause,
                    monthly_limit: limits.monthly_limit,
                },
            ),
            Some(_) => return Err(CostAdminError::Conflict),
            None => (
                "stop",
                CostLimits {
                    enabled: false,
                    ..old
                },
            ),
        };
        if next == old {
            tx.commit().await?;
            return Ok(());
        }
        // Audit and policy update are one transaction. Record only numeric
        // values and actor ID, no session token/digest, IP or email.
        let new_revision = old_revision
            .checked_add(1)
            .ok_or(CostAdminError::Unavailable)?;
        sqlx::query(
            "INSERT INTO mail_cost_admin_audit              (actor_user_id,action,old_revision,new_revision,old_enabled,new_enabled,              old_warning_1,old_warning_2,old_news_pause,old_monthly_limit,              new_warning_1,new_warning_2,new_news_pause,new_monthly_limit)              VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(actor)
        .bind(action)
        .bind(old_revision)
        .bind(new_revision)
        .bind(old.enabled)
        .bind(next.enabled)
        .bind(old.warning_1)
        .bind(old.warning_2)
        .bind(old.news_pause)
        .bind(old.monthly_limit)
        .bind(next.warning_1)
        .bind(next.warning_2)
        .bind(next.news_pause)
        .bind(next.monthly_limit)
        .execute(&mut *tx)
        .await?;
        let changed = sqlx::query(
            "UPDATE mail_cost_policy SET enabled=$1,warning_1=$2,warning_2=$3,             news_pause=$4,monthly_limit=$5,revision=$6 WHERE singleton AND revision=$7",
        )
        .bind(next.enabled)
        .bind(next.warning_1)
        .bind(next.warning_2)
        .bind(next.news_pause)
        .bind(next.monthly_limit)
        .bind(new_revision)
        .bind(old_revision)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(CostAdminError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn operator(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
) -> Result<i64, CostAdminError> {
    // Same live session/role fence as the existing S3 operator adapter.
    sqlx::query_scalar(
        "SELECT u.id FROM users u          JOIN sessions s ON s.user_id=u.id          JOIN account_roles r ON r.user_id=u.id AND r.role='administrator' AND r.is_active          WHERE s.token_hash=$1 AND s.revoked_at IS NULL            AND s.expires_at>clock_timestamp() AND u.account_state='active'          FOR SHARE OF s,u,r",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(CostAdminError::Forbidden)
}

async fn lock_guard(tx: &mut Transaction<'_, Postgres>) -> Result<(), CostAdminError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT singleton FROM mail_send_budget_guard WHERE singleton FOR UPDATE",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(())
}

async fn revision(tx: &mut Transaction<'_, Postgres>) -> Result<i64, CostAdminError> {
    Ok(
        sqlx::query_scalar("SELECT revision FROM mail_cost_policy WHERE singleton")
            .fetch_one(&mut **tx)
            .await?,
    )
}

mod reconciliation;
pub use reconciliation::MonthReconciliation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_must_be_ordered_positive_and_bounded() {
        let normal = LimitChange {
            warning_1: 10000,
            warning_2: 30000,
            news_pause: 40000,
            monthly_limit: 50000,
        };
        assert!(normal.valid());
        assert!(!LimitChange {
            warning_1: 0,
            ..normal
        }
        .valid());
        assert!(!LimitChange {
            warning_2: 50000,
            ..normal
        }
        .valid());
        assert!(!LimitChange {
            news_pause: 50000,
            ..normal
        }
        .valid());
        assert!(!LimitChange {
            monthly_limit: i64::MAX,
            ..normal
        }
        .valid());
    }
}
