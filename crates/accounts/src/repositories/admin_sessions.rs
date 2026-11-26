//! AUTH-04a: administrator session revocation and atomic security audit.
use super::PgAccounts;
use crate::services::security::VerifiedPassword;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminSessionRevokeError {
    Denied,
    InvalidTarget,
    StepUpRequired,
    Unavailable,
}

impl From<sqlx::Error> for AdminSessionRevokeError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

fn canonical_session_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

impl PgAccounts {
    /// Revoke one exact owned session or every live session of a different
    /// account. Actor is always derived from a live administrator cookie.
    /// The verified password capability must still match the locked account.
    /// A zero-row retry is successful and does not produce duplicate audits.
    pub async fn revoke_administrator_target_sessions(
        &self,
        administrator_digest: &[u8; 32],
        target_user_id: i64,
        selected_session: Option<&str>,
        password: &VerifiedPassword,
    ) -> Result<u64, AdminSessionRevokeError> {
        if !password.is_fresh() {
            return Err(AdminSessionRevokeError::StepUpRequired);
        }
        let mut tx = self.pool().begin().await?;
        sqlx::query(
            "SELECT set_config('statement_timeout','1500ms',true),
                    set_config('lock_timeout','250ms',true)",
        )
        .execute(&mut *tx)
        .await?;
        // Use the same authority lock as role grants/revocations. Acquire
        // both account rows in ascending order before any session row.
        sqlx::query("SELECT pg_advisory_xact_lock(18001,1)")
            .execute(&mut *tx)
            .await?;
        let actor: Option<i64> =
            sqlx::query_scalar("SELECT user_id FROM sessions WHERE token_hash=$1")
                .bind(administrator_digest.as_slice())
                .fetch_optional(&mut *tx)
                .await?;
        let actor = actor.ok_or(AdminSessionRevokeError::Denied)?;
        let locked: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 OR id=$2 ORDER BY id FOR UPDATE",
        )
        .bind(actor)
        .bind(target_user_id)
        .fetch_all(&mut *tx)
        .await?;
        let hash: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id=$1 AND account_state='active'",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await?;
        let hash = hash.ok_or(AdminSessionRevokeError::Denied)?;
        let session: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE token_hash=$1 AND user_id=$2
               AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR UPDATE",
        )
        .bind(administrator_digest.as_slice())
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await?;
        let role: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM account_roles
             WHERE user_id=$1 AND role='administrator' AND is_active FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await?;
        if session != Some(actor) || role != Some(actor) {
            return Err(AdminSessionRevokeError::Denied);
        }
        if !password.is_fresh() || !password.matches_hash(&hash) {
            return Err(AdminSessionRevokeError::StepUpRequired);
        }
        // Use the normal account-security path for self-revocation.
        if target_user_id <= 0
            || target_user_id == actor
            || !locked.contains(&target_user_id)
            || selected_session.is_some_and(|id| !canonical_session_id(id))
        {
            return Err(AdminSessionRevokeError::InvalidTarget);
        }
        let revoked = sqlx::query(
            "UPDATE sessions SET revoked_at=clock_timestamp()
             WHERE user_id=$1 AND revoked_at IS NULL AND expires_at>clock_timestamp()
               AND ($2::text IS NULL OR session_id::text=$2)",
        )
        .bind(target_user_id)
        .bind(selected_session)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if revoked > 0 {
            sqlx::query(
                "INSERT INTO account_admin_session_revocation_events
                 (administrator_user_id,target_user_id,selected_session_id,revoked_count)
                 VALUES($1,$2,$3::text::uuid,$4)",
            )
            .bind(actor)
            .bind(target_user_id)
            .bind(selected_session)
            .bind(i64::try_from(revoked).map_err(|_| AdminSessionRevokeError::Unavailable)?)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(revoked)
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_session_id;

    #[test]
    fn rejects_noncanonical_or_injected_identifiers() {
        assert!(canonical_session_id("3f6bce1a-65d6-4c3f-9001-2390c70cf711"));
        assert!(!canonical_session_id("3f6bce1a-65d6-4c3f-9001-2390c70cf71z"));
        assert!(!canonical_session_id("3f6bce1a-65d6-4c3f-9001-2390c70cf711' OR true"));
    }
}
