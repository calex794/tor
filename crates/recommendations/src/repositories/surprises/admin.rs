//! Audited administration of non-monetary editorial surprises.
//! Both the HTTP layer and every SQL transaction independently verify live A
//! administrator authority before mutating F-owned records.
use super::{PgSurprises, SurpriseError};
use sqlx::{Postgres, Row, Transaction};

#[derive(Debug)]
pub struct AdminSurprise {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub book_id: Option<i64>,
    pub editorial: bool,
}

/// Operational counts only: no raw page, click, impression, purchase or read log.
/// An arm is absent until 20 distinct accounts are measured in the period.
#[derive(Debug)]
pub struct ExperimentSummary {
    pub arm: String,
    pub unique_accounts: i64,
    pub evaluated_days: i64,
    pub planned: i64,
    pub reserved_slots: i64,
    pub opens: i64,
}

pub struct NewSurprise<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub book_id: Option<i64>,
    pub editorial: bool,
}

fn valid_content(input: &NewSurprise<'_>) -> bool {
    ["joke", "quote", "book", "fact"].contains(&input.kind)
        && !input.title.trim().is_empty()
        && input.title.trim() == input.title
        && input.title.chars().count() <= 120
        && !input.body.trim().is_empty()
        && input.body.trim() == input.body
        && input.body.chars().count() <= 600
        && (!["book", "quote"].contains(&input.kind) || input.book_id.is_some())
        && input.book_id.is_none_or(|id| id > 0)
}

async fn live_admin(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
) -> Result<i64, SurpriseError> {
    let actor: Option<i64> = sqlx::query_scalar(
        "SELECT u.id FROM sessions s
         JOIN users u ON u.id=s.user_id AND u.account_state='active'
         JOIN account_roles r ON r.user_id=u.id AND r.role='administrator' AND r.is_active
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
         FOR SHARE OF s,u,r",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    actor.ok_or(SurpriseError::Forbidden)
}

impl PgSurprises {
    pub async fn admin_overview(
        &self,
        digest: &[u8; 32],
    ) -> Result<(bool, i16, bool, Vec<AdminSurprise>), SurpriseError> {
        let mut tx = self.pool.begin().await?;
        live_admin(&mut tx, digest).await?;
        let (enabled, probability, experiment_enabled): (bool, i16, bool) = sqlx::query_as(
            "SELECT enabled,probability_per_mille,experiment_enabled
             FROM surprise_runtime_policy WHERE singleton",
        )
        .fetch_one(&mut *tx)
        .await?;
        let rows = sqlx::query(
            "SELECT id,kind,title,state,book_id,editorial FROM surprise_content
             ORDER BY created_at DESC,id DESC LIMIT 100",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let content = rows
            .into_iter()
            .map(|row| {
                Ok(AdminSurprise {
                    id: row.try_get("id")?,
                    kind: row.try_get("kind")?,
                    title: row.try_get("title")?,
                    state: row.try_get("state")?,
                    book_id: row.try_get("book_id")?,
                    editorial: row.try_get("editorial")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?;
        Ok((enabled, probability, experiment_enabled, content))
    }

    /// Last 7 complete UTC days, only arm totals and privacy-suppressed
    /// cohorts. These operational counters are NOT real icon impressions
    /// and cannot establish a causal effect on reading.
    pub async fn admin_experiment_summary(
        &self,
        digest: &[u8; 32],
    ) -> Result<Vec<ExperimentSummary>, SurpriseError> {
        let mut tx = self.pool.begin().await?;
        live_admin(&mut tx, digest).await?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(
            "SELECT experiment_arm,
                    count(DISTINCT user_id)::bigint AS unique_accounts,
                    count(*)::bigint AS evaluated_days,
                    count(*) FILTER(WHERE state='planned')::bigint AS planned,
                    count(*) FILTER(WHERE state IN ('claimed','opened','dismissed'))::bigint AS reserved_slots,
                    count(*) FILTER(WHERE state='opened')::bigint AS opens
             FROM surprise_daily_decisions
             WHERE experiment_arm IN ('control','gentle','standard')
               AND utc_day >= ((now() AT TIME ZONE 'UTC')::date-DATE '1970-01-01')::bigint-7
               AND utc_day < ((now() AT TIME ZONE 'UTC')::date-DATE '1970-01-01')::bigint
             GROUP BY experiment_arm
             HAVING count(DISTINCT user_id)>=20
             ORDER BY experiment_arm",
        )
        .fetch_all(&mut *tx)
        .await?;
        let items = rows
            .into_iter()
            .map(|row| {
                Ok(ExperimentSummary {
                    arm: row.try_get("experiment_arm")?,
                    unique_accounts: row.try_get("unique_accounts")?,
                    evaluated_days: row.try_get("evaluated_days")?,
                    planned: row.try_get("planned")?,
                    reserved_slots: row.try_get("reserved_slots")?,
                    opens: row.try_get("opens")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        tx.commit().await?;
        Ok(items)
    }

    pub async fn admin_create(
        &self,
        digest: &[u8; 32],
        input: &NewSurprise<'_>,
    ) -> Result<i64, SurpriseError> {
        if !valid_content(input) {
            return Err(SurpriseError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let actor = live_admin(&mut tx, digest).await?;
        if let Some(book) = input.book_id {
            let allowed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM books b
                 LEFT JOIN book_recommendation_audiences a ON a.book_id=b.id
                 WHERE b.id=$1 AND b.visibility='public' AND b.visibility_status='active'
                   AND b.lifecycle_status<>'pending_deletion'
                   AND NOT b.admin_hold AND NOT b.is_deleted
                   AND b.access_level='basic'
                   AND COALESCE(a.audience,'general')='general')",
            )
            .bind(book)
            .fetch_one(&mut *tx)
            .await?;
            if !allowed {
                return Err(SurpriseError::Forbidden);
            }
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO surprise_content(kind,title,body,book_id,editorial)
             VALUES($1,$2,$3,$4,$5) RETURNING id",
        )
        .bind(input.kind)
        .bind(input.title)
        .bind(input.body)
        .bind(input.book_id)
        .bind(input.editorial)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO surprise_admin_events(actor_user_id,action,content_id)
             VALUES($1,'create',$2)",
        )
        .bind(actor)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn admin_state(
        &self,
        digest: &[u8; 32],
        content_id: i64,
        next: &str,
    ) -> Result<bool, SurpriseError> {
        if content_id <= 0 || !["approved", "paused", "archived"].contains(&next) {
            return Err(SurpriseError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let actor = live_admin(&mut tx, digest).await?;
        // Only a freshly authored draft may become approved. A disabled row
        // cannot reactivate without an explicit new review.
        let affected = sqlx::query(
            "UPDATE surprise_content SET state=$2 WHERE id=$1
             AND ((state='draft' AND $2='approved')
               OR (state='approved' AND $2 IN ('paused','archived'))
               OR (state='paused' AND $2='archived'))",
        )
        .bind(content_id)
        .bind(next)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected != 0 {
            sqlx::query(
                "INSERT INTO surprise_admin_events(actor_user_id,action,content_id,details)
                 VALUES($1,'set_state',$2,jsonb_build_object('state',$3::text))",
            )
            .bind(actor)
            .bind(content_id)
            .bind(next)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(affected == 1)
    }

    pub async fn admin_policy(
        &self,
        digest: &[u8; 32],
        enabled: bool,
        probability: i16,
        experiment_enabled: bool,
    ) -> Result<(), SurpriseError> {
        if !(0..=1000).contains(&probability) {
            return Err(SurpriseError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let actor = live_admin(&mut tx, digest).await?;
        sqlx::query(
            "UPDATE surprise_runtime_policy
             SET enabled=$1,probability_per_mille=$2,experiment_enabled=$3,
                 revision=revision+1,updated_at=now() WHERE singleton",
        )
        .bind(enabled)
        .bind(probability)
        .bind(experiment_enabled)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO surprise_admin_events(actor_user_id,action,details)
             VALUES($1,'set_policy',jsonb_build_object('enabled',$2::boolean,'probability',$3::smallint,'experiment_enabled',$4::boolean))",
        )
        .bind(actor)
        .bind(enabled)
        .bind(probability)
        .bind(experiment_enabled)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
