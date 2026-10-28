//! PostgreSQL-owned referral persistence. No financial posting is allowed here.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgReferrals {
    pool: PgPool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureResult {
    Saved,
    Existing,
    Invalid,
}

impl PgReferrals {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn active_code(&self, code: &str) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM referral_codes WHERE code=$1 AND status='active')",
        )
        .bind(code)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn capture(
        &self,
        guest_hash: &[u8; 32],
        code: &str,
        source_kind: &str,
        source_path: &str,
    ) -> Result<CaptureResult, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // An active capture cannot be replaced. Clear expired rows before the
        // unique index check, under the same guest-scoped transaction lock.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(encode($1::bytea,'hex'),0))")
            .bind(guest_hash.as_slice())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE referral_captures SET status='expired' \
             WHERE guest_hash=$1 AND status='active' AND expires_at <= now()",
        )
        .bind(guest_hash.as_slice())
        .execute(&mut *tx)
        .await?;
        let previous: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM referral_captures WHERE guest_hash=$1 AND status='active'",
        )
        .bind(guest_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if previous.is_some() {
            tx.commit().await?;
            return Ok(CaptureResult::Existing);
        }
        let created: Option<i64> = sqlx::query_scalar(
            "INSERT INTO referral_captures(guest_hash,referral_code,source_kind,source_path) \
             SELECT $1,code,$3,$4 FROM referral_codes \
             WHERE code=$2 AND status='active' RETURNING id",
        )
        .bind(guest_hash.as_slice())
        .bind(code)
        .bind(source_kind)
        .bind(source_path)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(if created.is_some() {
            CaptureResult::Saved
        } else {
            CaptureResult::Invalid
        })
    }

    /// Durable A-owned outbox is always committed by account registration.
    /// F consumes the row with transaction and row lock: a replay cannot
    /// change the selected inviter or create duplicate attributions.
    pub async fn finalize_one(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let pending = sqlx::query(
            "SELECT guest_hash,manual_code FROM account_registration_referral_outbox \
             WHERE user_id=$1 AND processed_at IS NULL FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(pending) = pending else {
            tx.commit().await?;
            return Ok(false);
        };
        let guest_hash: Option<Vec<u8>> = pending.try_get("guest_hash")?;
        let manual: Option<String> = pending.try_get("manual_code")?;
        let candidate = if let Some(ref code) = manual {
            let manual_candidate = sqlx::query(
                "SELECT c.code,c.owner_user_id,c.program_type,NULL::bigint AS capture_id \
                 FROM referral_codes c \
                 WHERE c.code=$1 AND c.status='active'",
            )
            .bind(code)
            .fetch_optional(&mut *tx)
            .await?;
            if manual_candidate.is_some() {
                manual_candidate.map(|row| (row, "manual_code_at_registration"))
            } else {
                None
            }
        } else {
            None
        };
        let candidate = if candidate.is_some() {
            candidate
        } else if let Some(ref hash) = guest_hash {
            sqlx::query(
                "SELECT c.code,c.owner_user_id,c.program_type,r.id AS capture_id \
                 FROM referral_captures r JOIN referral_codes c ON c.code=r.referral_code \
                 WHERE r.guest_hash=$1 AND r.status='active' AND r.expires_at > now() \
                   AND c.status='active' LIMIT 1",
            )
            .bind(hash)
            .fetch_optional(&mut *tx)
            .await?
            .map(|row| (row, "guest_capture"))
        } else {
            None
        };
        let mut attributed = false;
        if let Some((row, source)) = candidate {
            let referrer: i64 = row.try_get("owner_user_id")?;
            if referrer != user_id {
                let code: String = row.try_get("code")?;
                let program: String = row.try_get("program_type")?;
                let capture: Option<i64> = row.try_get("capture_id")?;
                let affected = sqlx::query(
                    "INSERT INTO referral_attributions \
                     (program_type,subject_user_id,referrer_user_id,referral_code, \
                      source_capture_id,attribution_source,attributed_at,expires_at) \
                     VALUES($1,$2,$3,$4,$5,$6,now(), \
                      CASE WHEN $1='creator_income' THEN now()+INTERVAL '365 days' ELSE NULL END) \
                     ON CONFLICT(program_type,subject_user_id) DO NOTHING",
                )
                .bind(&program)
                .bind(user_id)
                .bind(referrer)
                .bind(code)
                .bind(capture)
                .bind(source)
                .execute(&mut *tx)
                .await?;
                attributed = affected.rows_affected() == 1;
            }
        }
        sqlx::query(
            "UPDATE account_registration_referral_outbox \
             SET processed_at=now(),result=$2 WHERE user_id=$1",
        )
        .bind(user_id)
        .bind(if attributed { "attributed" } else { "unattributed" })
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(attributed)
    }

    /// Bounded recovery for process crashes and delayed signups.
    pub async fn finalize_pending(&self, limit: i64) -> Result<u64, sqlx::Error> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT user_id FROM account_registration_referral_outbox \
             WHERE processed_at IS NULL ORDER BY registered_at,user_id LIMIT $1",
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?;
        for id in &ids {
            self.finalize_one(*id).await?;
        }
        Ok(ids.len() as u64)
    }
}
