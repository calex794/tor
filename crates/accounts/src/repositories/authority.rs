use super::PgAccounts;
use tor_contracts::auth::{AccountRole, Actor, SessionActor, UserId};

#[derive(Debug)]
pub enum AuthorityError {
    Denied,
    LastAdministrator,
    Unavailable,
}
impl From<sqlx::Error> for AuthorityError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

impl PgAccounts {
    pub async fn session_actor(
        &self,
        digest: &[u8; 32],
    ) -> Result<Option<SessionActor>, sqlx::Error> {
        let row: Option<(i64,bool,bool)> = sqlx::query_as(
            "SELECT u.id,
               EXISTS(SELECT 1 FROM account_roles r WHERE r.user_id=u.id AND r.role='author' AND r.is_active),
               EXISTS(SELECT 1 FROM account_roles r WHERE r.user_id=u.id AND r.role='administrator' AND r.is_active)
             FROM users u JOIN sessions s ON s.user_id=u.id
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now() AND u.account_state='active'",
        ).bind(digest.as_slice()).fetch_optional(self.pool()).await?;
        Ok(row.and_then(|(id, author, administrator)| {
            UserId::new(id).ok().map(|user_id| SessionActor {
                actor: Actor { user_id },
                author,
                administrator,
            })
        }))
    }

    /// H/I consume this port with the current session proof, not a claimed admin ID.
    /// Role revocation serializes with grants and is visible on the next call.
    pub async fn change_account_role(
        &self,
        session: &[u8; 32],
        target: UserId,
        role: AccountRole,
        enabled: bool,
    ) -> Result<(), AuthorityError> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(18001,1)")
            .execute(&mut *tx)
            .await?;
        let actor: Option<i64> =
            sqlx::query_scalar("SELECT user_id FROM sessions WHERE token_hash=$1")
                .bind(session.as_slice())
                .fetch_optional(&mut *tx)
                .await?;
        let actor = actor.ok_or(AuthorityError::Denied)?;
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            return Err(AuthorityError::Denied);
        }
        let live: Option<i64> = sqlx::query_scalar("SELECT user_id FROM sessions WHERE token_hash=$1 AND revoked_at IS NULL AND expires_at>now() FOR SHARE")
            .bind(session.as_slice()).fetch_optional(&mut *tx).await?;
        let admin: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_roles WHERE user_id=$1 AND role='administrator' AND is_active)")
            .bind(actor).fetch_one(&mut *tx).await?;
        if live != Some(actor) || !admin {
            return Err(AuthorityError::Denied);
        }
        if role == AccountRole::Administrator && !enabled {
            let others: i64=sqlx::query_scalar("SELECT count(*) FROM account_roles r JOIN users u ON u.id=r.user_id WHERE r.role='administrator' AND r.is_active AND u.account_state='active' AND r.user_id<>$1")
                .bind(target.get()).fetch_one(&mut *tx).await?;
            if others == 0 {
                return Err(AuthorityError::LastAdministrator);
            }
        }
        sqlx::query("INSERT INTO account_roles(user_id,role,is_active) VALUES($1,$2,$3) ON CONFLICT(user_id,role) DO UPDATE SET is_active=EXCLUDED.is_active")
            .bind(target.get()).bind(role.as_str()).bind(enabled).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO account_role_events(user_id,role,value,changed_by) VALUES($1,$2,$3,$4)",
        )
        .bind(target.get())
        .bind(role.as_str())
        .bind(if enabled { "granted" } else { "revoked" })
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
