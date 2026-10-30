//! PostgreSQL persistence for identities and revocable session lookups.
//! This adapter never stores plaintext passwords or session tokens.
#![forbid(unsafe_code)]

use crate::domain::access_audit::LoginAuditContext;
use sqlx::{PgPool, Row};
use std::num::NonZeroU32;

pub(super) const INSERT_AUDITED_SESSION: &str = "WITH saved AS ( \
    INSERT INTO sessions (user_id, token_hash, expires_at) \
    VALUES ($1, $2, now() + ($3::double precision * INTERVAL '1 second')) \
    RETURNING session_id, user_id \
) INSERT INTO account_login_events \
    (session_id, user_id, verified_ip, agent_family, event_kind) \
  SELECT session_id, user_id, $4::text::inet, $5, $6 FROM saved";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountWriteError {
    Duplicate,
    Unavailable,
    InvalidReferral,
}

impl From<sqlx::Error> for AccountWriteError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::Database(ref db) if db.code().as_deref() == Some("23505") => {
                Self::Duplicate
            }
            _ => Self::Unavailable,
        }
    }
}

pub struct RegistrationReferral<'a> {
    pub guest_hash: Option<&'a [u8; 32]>,
    pub manual_code: Option<&'a str>,
}

/// Registration audit and optional referral decision share one transaction.
pub struct RegistrationContext<'a> {
    pub referral: RegistrationReferral<'a>,
    pub audit: LoginAuditContext,
}

#[derive(Clone)]
pub struct PgAccounts {
    pool: PgPool,
}

pub struct StoredUser {
    pub id: i64,
    pub email_normalized: String,
    pub display_name: String,
    pub password_hash: String,
}

impl PgAccounts {
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Called before enabling account forms. Runtime may never connect as migrator.
    pub async fn probe(&self) -> Result<(), sqlx::Error> {
        let role: String = sqlx::query_scalar("SELECT current_user")
            .fetch_one(&self.pool)
            .await?;
        if role != "tor_app" {
            return Err(sqlx::Error::Protocol(
                "Accounts runtime requires tor_app, not tor_migrator".to_owned(),
            ));
        }
        sqlx::query("SELECT id FROM users LIMIT 0")
            .execute(&self.pool)
            .await?;
        sqlx::query("SELECT token_hash, session_id FROM sessions LIMIT 0")
            .execute(&self.pool)
            .await?;
        sqlx::query("SELECT session_id FROM account_login_events LIMIT 0")
            .execute(&self.pool)
            .await?;
        sqlx::query("SELECT key_hash FROM auth_attempts LIMIT 0")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A concurrent duplicate is rejected by PostgreSQL's unique constraint.
    pub async fn insert_user(
        &self,
        normalized_email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO users (email_normalized, display_name, password_hash) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(normalized_email)
        .bind(display_name)
        .bind(password_hash)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn find_user(
        &self,
        normalized_email: &str,
    ) -> Result<Option<StoredUser>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, email_normalized, display_name, password_hash \
             FROM users WHERE email_normalized = $1 AND account_state='active'",
        )
        .bind(normalized_email)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(StoredUser {
                id: r.try_get("id")?,
                email_normalized: r.try_get("email_normalized")?,
                display_name: r.try_get("display_name")?,
                password_hash: r.try_get("password_hash")?,
            })
        })
        .transpose()
    }

    /// Accepts a SHA-256 token digest, never the raw token; expiry uses DB time.
    pub async fn insert_session(
        &self,
        user_id: i64,
        digest: &[u8; 32],
        ttl_seconds: NonZeroU32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO sessions (user_id, token_hash, expires_at) \
             VALUES ($1, $2, now() + ($3::double precision * INTERVAL '1 second'))",
        )
        .bind(user_id)
        .bind(digest.as_slice())
        .bind(ttl_seconds.get() as f64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_audited_session(
        &self,
        user_id: i64,
        digest: &[u8; 32],
        ttl_seconds: NonZeroU32,
        audit: LoginAuditContext,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(INSERT_AUDITED_SESSION)
            .bind(user_id)
            .bind(digest.as_slice())
            .bind(ttl_seconds.get() as f64)
            .bind(audit.verified_ip.map(|ip| ip.to_string()))
            .bind(audit.agent_family.label())
            .bind(audit.event_kind.label())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn active_session_user(&self, digest: &[u8; 32]) -> Result<Option<i64>, sqlx::Error> {
        Ok(self
            .session_actor(digest)
            .await?
            .map(|actor| actor.actor.user_id.get()))
    }

    /// Register and establish the first session atomically. A failed session
    /// write rolls back user creation, avoiding a stranded registration.
    pub async fn register_with_session(
        &self,
        email: &str,
        name: &str,
        password_hash: &str,
        digest: &[u8; 32],
        ttl: NonZeroU32,
    ) -> Result<i64, AccountWriteError> {
        let mut transaction = self.pool.begin().await?;
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (email_normalized, display_name, password_hash) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO sessions (user_id, token_hash, expires_at) \
             VALUES ($1, $2, now() + ($3::double precision * INTERVAL '1 second'))",
        )
        .bind(user_id)
        .bind(digest.as_slice())
        .bind(ttl.get() as f64)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(user_id)
    }

    pub async fn register_with_audited_session(
        &self,
        email: &str,
        name: &str,
        password_hash: &str,
        digest: &[u8; 32],
        ttl: NonZeroU32,
        audit: LoginAuditContext,
    ) -> Result<i64, AccountWriteError> {
        let mut tx = self.pool.begin().await?;
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (email_normalized, display_name, password_hash) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(INSERT_AUDITED_SESSION)
            .bind(user_id)
            .bind(digest.as_slice())
            .bind(ttl.get() as f64)
            .bind(audit.verified_ip.map(|ip| ip.to_string()))
            .bind(audit.agent_family.label())
            .bind(audit.event_kind.label())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(user_id)
    }

    /// Referrals share the same commit as the account and its initial session.
    /// This extension keeps the original five-argument registration API intact.
    pub async fn register_with_audited_session_referral(
        &self,
        email: &str,
        name: &str,
        password_hash: &str,
        digest: &[u8; 32],
        ttl: NonZeroU32,
        context: RegistrationContext<'_>,
    ) -> Result<i64, AccountWriteError> {
        let RegistrationContext { referral, audit } = context;
        let mut transaction = self.pool.begin().await?;
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (email_normalized, display_name, password_hash) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(INSERT_AUDITED_SESSION)
            .bind(user_id)
            .bind(digest.as_slice())
            .bind(ttl.get() as f64)
            .bind(audit.verified_ip.map(|ip| ip.to_string()))
            .bind(audit.agent_family.label())
            .bind(audit.event_kind.label())
            .execute(&mut *transaction)
            .await?;
        if referral.guest_hash.is_some() || referral.manual_code.is_some() {
            // Freeze the winner at the moment the account and session commit.
            // Reading this information later loses valid invitations if a
            // code is disabled or an earlier guest capture changes status.
            let mut candidate = if let Some(code) = referral.manual_code {
                sqlx::query(
                    "SELECT code,owner_user_id,program_type,
                            NULL::bigint AS capture_id,
                            'manual_code_at_registration'::text AS source
                     FROM referral_codes
                     WHERE code=$1 AND status='active' AND owner_user_id<>$2
                     FOR SHARE",
                )
                .bind(code)
                .bind(user_id)
                .fetch_optional(&mut *transaction)
                .await?
            } else {
                None
            };
            if referral.manual_code.is_some() && candidate.is_none() {
                return Err(AccountWriteError::InvalidReferral);
            }
            // A guest capture belongs to at most one committed registration.
            // Claim it under a row lock *inside* the account/session transaction:
            // a concurrent signup with a copied guest cookie cannot claim it.
            // Manual codes retain precedence, but successful signup still
            // consumes its guest token's capture (even when manual wins).
            if let Some(hash) = referral.guest_hash {
                let claimed: Option<(i64, String)> = sqlx::query_as(
                    "SELECT id,referral_code FROM referral_captures
                     WHERE guest_hash=$1 AND status='active'
                       AND captured_at<=now() AND expires_at>now()
                     FOR UPDATE",
                )
                .bind(hash.as_slice())
                .fetch_optional(&mut *transaction)
                .await?;
                if let Some((capture_id, guest_code)) = claimed {
                    if candidate.is_none() {
                        candidate = sqlx::query(
                            "SELECT code,owner_user_id,program_type,
                                    $2::bigint AS capture_id,
                                    'guest_capture'::text AS source
                             FROM referral_codes
                             WHERE code=$1 AND status='active'
                               AND owner_user_id<>$3 FOR SHARE",
                        )
                        .bind(&guest_code)
                        .bind(capture_id)
                        .bind(user_id)
                        .fetch_optional(&mut *transaction)
                        .await?;
                    }
                    // This write rolls back if the session or outbox fails.
                    // The selected sponsor is snapshotted below for recovery;
                    // finalization must not claim the guest a second time.
                    let spent = sqlx::query(
                        "UPDATE referral_captures SET status='consumed'
                         WHERE id=$1 AND status='active'",
                    )
                    .bind(capture_id)
                    .execute(&mut *transaction)
                    .await?;
                    if spent.rows_affected() != 1 {
                        // Never commit an outbox decision if the guest bearer
                        // was not actually spent by the same transaction.
                        return Err(AccountWriteError::Unavailable);
                    }
                }
            }
            let (code, owner, program, capture, source) = match candidate {
                Some(row) => (
                    Some(row.try_get::<String, _>("code")?),
                    Some(row.try_get::<i64, _>("owner_user_id")?),
                    Some(row.try_get::<String, _>("program_type")?),
                    row.try_get::<Option<i64>, _>("capture_id")?,
                    Some(row.try_get::<String, _>("source")?),
                ),
                None => (None, None, None, None, None),
            };
            sqlx::query(
                "INSERT INTO account_registration_referral_outbox(
                    user_id,guest_hash,manual_code,decided_at_registration,
                    decided_referral_code,decided_referrer_user_id,
                    decided_program_type,decided_capture_id,decided_source
                 ) VALUES($1,$2,$3,TRUE,$4,$5,$6,$7,$8)",
            )
            .bind(user_id)
            .bind(referral.guest_hash.map(|hash| hash.as_slice()))
            .bind(referral.manual_code)
            .bind(code)
            .bind(owner)
            .bind(program)
            .bind(capture)
            .bind(source)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(user_id)
    }

    /// Read the minimal private profile, never return password hashes to HTML.
    pub async fn user_display_name(&self, user_id: i64) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Atomic counter prevents concurrent attempts bypassing limits.
    /// Counters are shared by all server processes; the caller hashes keys.
    pub async fn permit_auth_attempt(
        &self,
        key: &[u8; 32],
        max_attempts: i32,
        window_seconds: i32,
    ) -> Result<bool, sqlx::Error> {
        let attempts: i32 = sqlx::query_scalar(
            "INSERT INTO auth_attempts AS rate (key_hash, attempts, period_started) \
             VALUES ($1, 1, now()) ON CONFLICT (key_hash) DO UPDATE SET \
             attempts = CASE WHEN rate.period_started + ($2::integer * INTERVAL '1 second') <= now() \
                  THEN 1 ELSE rate.attempts + 1 END, \
             period_started = CASE WHEN rate.period_started + ($2::integer * INTERVAL '1 second') <= now() \
                  THEN now() ELSE rate.period_started END \
             RETURNING attempts",
        )
        .bind(key.as_slice())
        .bind(window_seconds)
        .fetch_one(&self.pool)
        .await?;
        Ok(attempts <= max_attempts)
    }

    /// Successful login resets the per-identity attempt counter only.
    pub async fn clear_auth_attempt(&self, key: &[u8; 32]) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM auth_attempts WHERE key_hash = $1")
            .bind(key.as_slice())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Safe to retry; session lookup must reject the same token immediately.
    /// Consent defaults OFF when no row is present; no assumed agreement.
    pub async fn ril_behavior_consent(&self, user_id: i64) -> Result<(bool, i64), sqlx::Error> {
        let row = sqlx::query_as::<_, (bool, i64)>(
            "SELECT enabled, epoch FROM ril_behavior_consents WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.unwrap_or((false, 0)))
    }

    /// Single DB-backed verification for the optional private GET observer.
    /// A caller cannot supply a user ID or convert an arbitrary cookie into identity.
    pub async fn ril_verified_opted_in_session(
        &self,
        token_digest: &[u8; 32],
    ) -> Result<Option<(i64, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT s.user_id, c.epoch FROM sessions s
                JOIN ril_behavior_consents c ON c.user_id = s.user_id
             WHERE s.token_hash = $1 AND s.revoked_at IS NULL
               AND s.expires_at > now() AND c.enabled = TRUE
               AND EXISTS (SELECT 1 FROM users u WHERE u.id=s.user_id AND u.account_state='active')",
        )
        .bind(token_digest.as_slice())
        .fetch_optional(&self.pool)
        .await
    }

    /// Monotonic epoch rotates the HMAC pseudonym on EVERY consent transition.
    /// A repeated disable leaves the epoch unchanged so memory erasure can retry.
    pub async fn set_ril_behavior_consent(
        &self,
        verified_user_id: i64,
        enabled: bool,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO ril_behavior_consents(user_id,enabled,epoch)
             VALUES ($1,$2,1)
             ON CONFLICT (user_id) DO UPDATE
                SET enabled = EXCLUDED.enabled,
                    epoch = CASE WHEN ril_behavior_consents.enabled = EXCLUDED.enabled
                                 THEN ril_behavior_consents.epoch
                                 ELSE ril_behavior_consents.epoch + 1 END,
                    updated_at = now()
             RETURNING epoch",
        )
        .bind(verified_user_id)
        .bind(enabled)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn revoke_session(&self, digest: &[u8; 32]) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE sessions SET revoked_at = now() \
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(digest.as_slice())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() != 0)
    }
}
