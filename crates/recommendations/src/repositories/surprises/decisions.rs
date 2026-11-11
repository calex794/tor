//! One server-owned chance per UTC day; every state-changing operation locks
//! the live A user and the F preference head in a fixed order.
use super::{PgSurprises, PlanState, SurpriseError};
use crate::services::surprise_schedule::{
    decide, experiment_arm, Decision, ExperimentArm, SurpriseSchedule,
};
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgSurprises {
    pub async fn plan(
        &self,
        user_id: i64,
        server_key: &[u8; 32],
    ) -> Result<PlanState, SurpriseError> {
        if user_id <= 0 {
            return Err(SurpriseError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            return Err(SurpriseError::Forbidden);
        }
        let (enabled, mut prob, experimental): (bool, i16, bool) = sqlx::query_as(
            "SELECT enabled,probability_per_mille,experiment_enabled
             FROM surprise_runtime_policy WHERE singleton",
        )
        .fetch_one(&mut *tx)
        .await?;
        let disabled: bool = sqlx::query_scalar(
            "SELECT COALESCE((SELECT disabled FROM surprise_user_preferences WHERE user_id=$1),false)",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if !enabled || disabled {
            return Ok(PlanState::Off);
        }
        let day: i64 = sqlx::query_scalar(
            "SELECT ((now() AT TIME ZONE 'UTC')::date - DATE '1970-01-01')::bigint",
        )
        .fetch_one(&mut *tx)
        .await?;
        let mut digest = Sha256::new();
        digest.update(b"tor-surprise-user-v1");
        digest.update(user_id.to_be_bytes());
        let subject: [u8; 32] = digest.finalize().into();
        let mut arm = None;
        let mut run = None;
        if experimental {
            // Enroll BEFORE the appearance draw. A stored assignment, not a
            // later click, key rotation or new day, owns this account's arm.
            let candidate = experiment_arm(server_key, &subject);
            let assigned: Option<(i64, String, i16)> =
                sqlx::query_as("SELECT run_id,arm,probability FROM surprise_enroll($1,$2,$3)")
                    .bind(user_id)
                    .bind(candidate.as_str())
                    .bind(day)
                    .fetch_optional(&mut *tx)
                    .await?;
            let Some((id, pinned, probability)) = assigned else {
                tx.commit().await?;
                return Ok(PlanState::NotSelected);
            };
            run = Some(id);
            prob = probability;
            arm = Some(match pinned.as_str() {
                "control" => ExperimentArm::Control,
                "gentle" => ExperimentArm::Gentle,
                "standard" => ExperimentArm::Standard,
                _ => return Err(SurpriseError::Unavailable),
            });
        }
        let appearance_per_mille = match arm {
            Some(ExperimentArm::Control) => 0_u16,
            Some(ExperimentArm::Gentle) => (prob as u16) / 2,
            Some(ExperimentArm::Standard) | None => prob as u16,
        };
        let scheduled = decide(
            server_key,
            &subject,
            day,
            SurpriseSchedule {
                enabled: true,
                probability_per_mille: appearance_per_mille,
            },
        )
        .map_err(|_| SurpriseError::Invalid)?;
        let (state, seconds, anchor, content) = match scheduled {
            Decision::Planned(plan) => (
                "planned",
                Some(plan.active_after_seconds as i32),
                Some(plan.anchor_ticket as i64),
                Some(plan.content_ticket as i64),
            ),
            Decision::NotSelected | Decision::Disabled => ("not_selected", None, None, None),
        };
        sqlx::query(
            "INSERT INTO surprise_daily_decisions(
                 user_id,utc_day,state,active_after_seconds,anchor_ticket,content_ticket,
                 experiment_arm,experiment_run_id)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (user_id,utc_day) DO NOTHING",
        )
        .bind(user_id)
        .bind(day)
        .bind(state)
        .bind(seconds)
        .bind(anchor)
        .bind(content)
        .bind(arm.map(ExperimentArm::as_str))
        .bind(run)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(
            "SELECT state,active_after_seconds,surprise_delivery_allowed(user_id,utc_day) AS allowed
             FROM surprise_daily_decisions WHERE user_id=$1 AND utc_day=$2",
        )
        .bind(user_id)
        .bind(day)
        .fetch_one(&mut *tx)
        .await?;
        let status: &str = row.try_get("state")?;
        let allowed: bool = row.try_get("allowed")?;
        let selected = if status == "planned" && allowed {
            PlanState::Planned {
                utc_day: day,
                active_after_seconds: row.try_get("active_after_seconds")?,
            }
        } else {
            PlanState::NotSelected
        };
        tx.commit().await?;
        Ok(selected)
    }

    /// Atomically consumes a maximum of one visible slot per sliding 24 hours.
    /// The server wall clock must corroborate the claimed active-time threshold.
    pub async fn claim(
        &self,
        user_id: i64,
        utc_day: i64,
        active_seconds: i32,
    ) -> Result<bool, SurpriseError> {
        if user_id <= 0 || utc_day <= 18000 || !(120..=86400).contains(&active_seconds) {
            return Err(SurpriseError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            return Err(SurpriseError::Forbidden);
        }
        sqlx::query(
            "INSERT INTO surprise_user_preferences(user_id) VALUES($1)
             ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        let allowed: bool = sqlx::query_scalar(
            "SELECT NOT p.disabled
                 AND (p.backoff_until IS NULL OR p.backoff_until<=now())
                 AND (p.last_shown_at IS NULL OR p.last_shown_at<=now()-interval '24 hours')
                 AND (SELECT enabled FROM surprise_runtime_policy WHERE singleton)
             FROM surprise_user_preferences p WHERE p.user_id=$1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if !allowed {
            return Ok(false);
        }
        let available: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM surprise_content c
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
            )",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !available {
            return Ok(false);
        }
        let changed = sqlx::query(
            "UPDATE surprise_daily_decisions SET state='claimed',claimed_at=now()
             WHERE user_id=$1 AND utc_day=$2 AND state='planned'
               AND surprise_delivery_allowed(user_id,utc_day)
               AND active_after_seconds<=$3
               AND created_at<=now()-active_after_seconds*interval '1 second'",
        )
        .bind(user_id)
        .bind(utc_day)
        .bind(active_seconds)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed == 0 {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE surprise_user_preferences
             SET last_shown_at=now(),updated_at=now() WHERE user_id=$1",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn disable(&self, user_id: i64) -> Result<(), SurpriseError> {
        if user_id <= 0 {
            return Err(SurpriseError::Invalid);
        }
        let changed = sqlx::query(
            "INSERT INTO surprise_user_preferences(user_id,disabled)
             SELECT id,true FROM users WHERE id=$1 AND account_state='active'
             ON CONFLICT (user_id) DO UPDATE SET disabled=true,updated_at=now()",
        )
        .bind(user_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed == 0 {
            return Err(SurpriseError::Forbidden);
        }
        Ok(())
    }
}
