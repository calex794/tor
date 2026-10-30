//! PostgreSQL-owned referral persistence. No financial posting is allowed here.
use sqlx::{postgres::PgRow, PgPool, Row};

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

/// A single, immutable default invite per reader and program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedReferralCode {
    pub code: String,
    pub status: String,
    pub alias: Option<String>,
    pub alias_path_prefix: Option<String>,
}

/// One default code may have at most one immutable vanity alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasClaim {
    Created,
    Unchanged,
    Invalid,
    Taken,
    Locked,
    Unavailable,
}

/// Only aggregate counters, never invited identities, emails, or money.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferralOverview {
    pub default_code: Option<OwnedReferralCode>,
    pub active: i64,
    pub under_review: i64,
    pub blocked: i64,
    pub expired: i64,
}

/// The payout owner and program are copied from signup-time evidence;
/// replay must never select a different sponsor from mutable code status.
struct ReferralCandidate {
    code: String,
    referrer: i64,
    program: String,
    capture: Option<i64>,
    source: String,
}

impl ReferralCandidate {
    fn from_row(row: PgRow, source: &str) -> Result<Self, sqlx::Error> {
        Ok(Self {
            code: row.try_get("code")?,
            referrer: row.try_get("owner_user_id")?,
            program: row.try_get("program_type")?,
            capture: row.try_get("capture_id")?,
            source: source.to_owned(),
        })
    }
}

impl PgReferrals {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// This read is actor-scoped; callers must supply a verified session ID.
    pub async fn default_reader_code(
        &self,
        owner_user_id: i64,
    ) -> Result<Option<OwnedReferralCode>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
            "SELECT code,status,alias,alias_path_prefix FROM referral_codes
             WHERE owner_user_id=$1 AND program_type='reader_spend' AND is_default=TRUE",
        )
        .bind(owner_user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(code, status, alias, alias_path_prefix)| OwnedReferralCode {
                code,
                status,
                alias,
                alias_path_prefix,
            },
        ))
    }

    /// A form retry or two simultaneous tabs must return the very same code.
    /// A disabled/blocked default cannot be replaced to bypass moderation.
    pub async fn create_default_reader_code(
        &self,
        owner_user_id: i64,
        proposed_code: &str,
    ) -> Result<OwnedReferralCode, sqlx::Error> {
        if !crate::domain::valid_code(proposed_code) {
            return Err(sqlx::Error::Protocol(
                "invalid generated invite code".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO referral_codes(code,owner_user_id,program_type,is_default)
             VALUES($1,$2,'reader_spend',TRUE) ON CONFLICT DO NOTHING",
        )
        .bind(proposed_code)
        .bind(owner_user_id)
        .execute(&self.pool)
        .await?;
        self.default_reader_code(owner_user_id)
            .await?
            .ok_or_else(|| sqlx::Error::Protocol("could not reserve referral code".into()))
    }

    /// Registration attribution is independent of any payout/reading event.
    /// Never return subject IDs or emails from this personal dashboard query.
    pub async fn reader_overview(
        &self,
        owner_user_id: i64,
    ) -> Result<ReferralOverview, sqlx::Error> {
        let default_code = self.default_reader_code(owner_user_id).await?;
        let (active, under_review, blocked, expired) = sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT count(*) FILTER (WHERE status='active'),
                        count(*) FILTER (WHERE status='under_review'),
                        count(*) FILTER (WHERE status='blocked'),
                        count(*) FILTER (WHERE status='expired')
                 FROM referral_attributions
                 WHERE referrer_user_id=$1 AND program_type='reader_spend'",
        )
        .bind(owner_user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(ReferralOverview {
            default_code,
            active,
            under_review,
            blocked,
            expired,
        })
    }

    /// A reader can reserve a single alias for their own active default code.
    /// The binding is permanent: changing it would let a former link's readers
    /// resolve to a different sponsor. Concurrent claims fail deterministically.
    pub async fn claim_reader_alias(
        &self,
        owner_user_id: i64,
        prefix: &str,
        alias: &str,
    ) -> Result<AliasClaim, sqlx::Error> {
        if !matches!(prefix, "go" | "promo") || !crate::domain::valid_alias(alias) {
            return Ok(AliasClaim::Invalid);
        }
        let claimed: Option<String> = match sqlx::query_scalar(
            "UPDATE referral_codes SET alias_path_prefix=$2,alias=$3
             WHERE owner_user_id=$1 AND program_type='reader_spend'
               AND is_default=TRUE AND status='active' AND alias IS NULL
             RETURNING code",
        )
        .bind(owner_user_id)
        .bind(prefix)
        .bind(alias)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(claimed) => claimed,
            Err(sqlx::Error::Database(error))
                if error.constraint() == Some("referral_codes_alias") =>
            {
                return Ok(AliasClaim::Taken);
            }
            Err(error) => return Err(error),
        };
        if claimed.is_some() {
            return Ok(AliasClaim::Created);
        }
        let Some(current) = self.default_reader_code(owner_user_id).await? else {
            return Ok(AliasClaim::Unavailable);
        };
        if current.status != "active" {
            return Ok(AliasClaim::Unavailable);
        }
        if current.alias.as_deref() == Some(alias)
            && current.alias_path_prefix.as_deref() == Some(prefix)
        {
            Ok(AliasClaim::Unchanged)
        } else if current.alias.is_some() {
            Ok(AliasClaim::Locked)
        } else {
            Ok(AliasClaim::Unavailable)
        }
    }

    /// An alias only points to an active code and never directly captures identity.
    pub async fn resolve_active_alias(
        &self,
        prefix: &str,
        alias: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        if !matches!(prefix, "go" | "promo") || !crate::domain::valid_alias(alias) {
            return Ok(None);
        }
        sqlx::query_scalar(
            "SELECT code FROM referral_codes
             WHERE alias_path_prefix=$1 AND alias=$2 AND status='active'",
        )
        .bind(prefix)
        .bind(alias)
        .fetch_optional(&self.pool)
        .await
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
        // A consumed or fraud-blocked capture remains a spent bearer for its
        // original 30-day window. Replaying a copied guest cookie cannot
        // create a second active capture after the first signup.
        let previous: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM referral_captures
             WHERE guest_hash=$1 AND status IN ('active','consumed','blocked')
               AND expires_at>now() LIMIT 1",
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

    /// Account registration commits the selected inviter with its outbox.
    /// A delayed consumer MUST replay that decision, not mutable code status.
    /// Old/test outbox rows without a snapshot use the historical resolver.
    pub async fn finalize_one(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let pending = sqlx::query(
            "SELECT guest_hash,manual_code,decided_at_registration,
                    decided_referral_code,decided_referrer_user_id,
                    decided_program_type,decided_capture_id,decided_source
             FROM account_registration_referral_outbox
             WHERE user_id=$1 AND processed_at IS NULL FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(pending) = pending else {
            tx.commit().await?;
            return Ok(false);
        };

        let candidate = if pending.try_get::<bool, _>("decided_at_registration")? {
            let code: Option<String> = pending.try_get("decided_referral_code")?;
            if let Some(code) = code {
                // CHECK constraints guarantee an all-or-nothing decision.
                let missing =
                    || sqlx::Error::Protocol("incomplete committed referral decision".to_owned());
                Some(ReferralCandidate {
                    code,
                    referrer: pending
                        .try_get::<Option<i64>, _>("decided_referrer_user_id")?
                        .ok_or_else(missing)?,
                    program: pending
                        .try_get::<Option<String>, _>("decided_program_type")?
                        .ok_or_else(missing)?,
                    capture: pending.try_get("decided_capture_id")?,
                    source: pending
                        .try_get::<Option<String>, _>("decided_source")?
                        .ok_or_else(missing)?,
                })
            } else {
                // No eligible invitation existed when the account committed.
                // Later clicks or codes must not retroactively claim the user.
                None
            }
        } else {
            let manual: Option<String> = pending.try_get("manual_code")?;
            let guest_hash: Option<Vec<u8>> = pending.try_get("guest_hash")?;
            let mut candidate = None;
            if let Some(code) = manual {
                candidate = sqlx::query(
                    "SELECT c.code,c.owner_user_id,c.program_type,
                            NULL::bigint AS capture_id
                     FROM referral_codes c
                     JOIN account_registration_referral_outbox o ON o.user_id=$2
                     WHERE c.code=$1 AND c.status='active'
                       AND c.created_at <= o.registered_at",
                )
                .bind(code)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| ReferralCandidate::from_row(row, "manual_code_at_registration"))
                .transpose()?;
            }
            if candidate.is_none() {
                if let Some(hash) = guest_hash {
                    candidate = sqlx::query(
                        "SELECT c.code,c.owner_user_id,c.program_type,
                                r.id AS capture_id
                         FROM referral_captures r
                         JOIN referral_codes c ON c.code=r.referral_code
                         JOIN account_registration_referral_outbox o ON o.user_id=$2
                         WHERE r.guest_hash=$1
                           AND r.status IN ('active','expired','consumed')
                           AND r.captured_at <= o.registered_at
                           AND r.expires_at > o.registered_at
                           AND c.status='active'
                         ORDER BY r.captured_at ASC,r.id ASC LIMIT 1",
                    )
                    .bind(hash)
                    .bind(user_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(|row| ReferralCandidate::from_row(row, "guest_capture"))
                    .transpose()?;
                }
            }
            candidate
        };

        // Keep the signup owner immutable, but a subsequently blocked code
        // or capture must never become an active/payout-eligible attribution.
        let mut attributed = false;
        if let Some(candidate) = candidate {
            if candidate.referrer != user_id {
                let affected = sqlx::query(
                    "INSERT INTO referral_attributions
                     (program_type,subject_user_id,referrer_user_id,referral_code,
                      source_capture_id,attribution_source,attributed_at,expires_at,status)
                     SELECT $1,$2,$3,$4,$5,$6,registered_at,
                       CASE WHEN $1='creator_income'
                         THEN registered_at + INTERVAL '365 days' ELSE NULL END,
                       CASE WHEN EXISTS(
                           SELECT 1 FROM referral_codes
                           WHERE code=$4 AND status='blocked'
                         ) OR EXISTS(
                           SELECT 1 FROM referral_captures
                           WHERE id=$5 AND status='blocked'
                         ) THEN 'blocked' ELSE 'active' END
                     FROM account_registration_referral_outbox WHERE user_id=$2
                     -- A capture is single-use even for legacy/replayed outbox.
                     -- Conflict on the source capture must settle the second
                     -- event as unattributed, not poison the retry worker.
                     ON CONFLICT DO NOTHING",
                )
                .bind(&candidate.program)
                .bind(user_id)
                .bind(candidate.referrer)
                .bind(&candidate.code)
                .bind(candidate.capture)
                .bind(&candidate.source)
                .execute(&mut *tx)
                .await?;
                attributed = affected.rows_affected() == 1;
            }
        }
        sqlx::query(
            "UPDATE account_registration_referral_outbox
             SET processed_at=now(),result=$2 WHERE user_id=$1",
        )
        .bind(user_id)
        .bind(if attributed {
            "attributed"
        } else {
            "unattributed"
        })
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
