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

pub struct AdminTargetSession {
    pub id: String,
    pub opened_utc: String,
    pub expires_utc: String,
    pub browser_family: String,
}

pub struct AdminSessionOverview {
    pub display_name: String,
    pub account_state: String,
    pub sessions: Vec<AdminTargetSession>,
}

type OverviewRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl PgAccounts {
    /// Exact target lookup, bounded to 50 live sessions, with the current
    /// administrator's session and role checked in the same SQL snapshot.
    pub async fn administrator_session_overview(
        &self,
        digest: &[u8; 32],
        target: i64,
    ) -> Result<Option<AdminSessionOverview>, sqlx::Error> {
        let rows = tokio::time::timeout(
            std::time::Duration::from_millis(950),
            self.read_administrator_session_overview(digest, target),
        )
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)??;
        let Some(first) = rows.first() else {
            return Ok(None);
        };
        let display_name = first.0.clone();
        let account_state = first.1.clone();
        let sessions = rows
            .into_iter()
            .filter_map(|(_, _, id, opened, expires, browser)| {
                Some(AdminTargetSession {
                    id: id?,
                    opened_utc: opened?,
                    expires_utc: expires?,
                    browser_family: browser?,
                })
            })
            .collect();
        Ok(Some(AdminSessionOverview {
            display_name,
            account_state,
            sessions,
        }))
    }

    async fn read_administrator_session_overview(
        &self,
        digest: &[u8; 32],
        target: i64,
    ) -> Result<Vec<OverviewRow>, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "SELECT set_config('statement_timeout','500ms',true),
                    set_config('lock_timeout','25ms',true)",
        )
        .execute(&mut *tx)
        .await?;
        let rows = sqlx::query_as(
            "SELECT target.display_name,target.account_state,
                    live.id,live.opened,live.expires,live.browser
             FROM users target
             JOIN sessions requester ON requester.token_hash=$1
             JOIN users administrator ON administrator.id=requester.user_id
             JOIN account_roles role ON role.user_id=administrator.id
             LEFT JOIN LATERAL (
                 SELECT s.session_id::text AS id,
                     to_char(s.created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI') AS opened,
                     to_char(s.expires_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI') AS expires,
                     COALESCE(e.agent_family,'unknown') AS browser
                 FROM sessions s
                 LEFT JOIN account_login_events e ON e.session_id=s.session_id
                     AND e.user_id=s.user_id
                 WHERE s.user_id=target.id AND s.revoked_at IS NULL AND s.expires_at>now()
                 ORDER BY s.created_at DESC,s.session_id DESC LIMIT 50
             ) live ON TRUE
             WHERE target.id=$2 AND target.id<>administrator.id
               AND requester.revoked_at IS NULL AND requester.expires_at>now()
               AND administrator.account_state='active'
               AND role.role='administrator' AND role.is_active",
        )
        .bind(digest.as_slice())
        .bind(target)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

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
