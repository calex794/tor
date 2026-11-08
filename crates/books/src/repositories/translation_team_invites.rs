//! S027: recipient-bound Team invites; membership never grants Book/D rights.
#![forbid(unsafe_code)]
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use super::translation_teams::TeamError;

#[derive(Clone)]
pub struct PgTeamInvites {
    pool: PgPool,
}
#[derive(Debug)]
pub struct TeamInvite {
    pub team_id: i64,
    pub team_name: String,
    pub invitee_user_id: i64,
    pub state: String,
}
fn db_error(e: sqlx::Error) -> TeamError {
    if let sqlx::Error::Database(ref db) = e {
        if db.code().as_deref() == Some("23505") { return TeamError::Conflict; }
    }
    TeamError::Unavailable
}
fn digest(token: &str) -> Result<Vec<u8>, TeamError> {
    if token.len() != 43 || !token.bytes().all(|c|
        c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
    ) { return Err(TeamError::Missing); }
    Ok(Sha256::digest(token.as_bytes()).to_vec())
}
impl PgTeamInvites {
    pub fn new(pool: PgPool) -> Self { Self { pool } }

    /// Caller supplies a CSPRNG-generated 43-character token; only its digest persists.
    pub async fn invite(&self, owner: i64, team_id: i64, recipient_email: &str, token: &str)
        -> Result<(), TeamError>
    {
        let email = recipient_email.trim().to_lowercase();
        if owner <= 0 || team_id <= 0 ||
            email.is_empty() || email.len() > 320 ||
            !email.contains('@') || email.chars().any(char::is_whitespace)
        { return Err(TeamError::Invalid); }
        let hash = digest(token).map_err(|_| TeamError::Invalid)?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let team_owner: Option<i64> = sqlx::query_scalar(
            "SELECT t.owner_user_id FROM translation_teams t
             JOIN users u ON u.id=t.owner_user_id AND u.account_state='active'
             WHERE t.id=$1 AND t.owner_user_id=$2 FOR UPDATE OF t")
            .bind(team_id).bind(owner)
            .fetch_optional(&mut *tx).await.map_err(db_error)?;
        if team_owner.is_none() { return Err(TeamError::Missing); }
        let recipient: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE email_normalized=$1 AND account_state='active'
             FOR SHARE")
            .bind(&email).fetch_optional(&mut *tx).await.map_err(db_error)?;
        let recipient = recipient.ok_or(TeamError::Missing)?;
        if recipient == owner { return Err(TeamError::Invalid); }
        let member: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM translation_team_members WHERE team_id=$1 AND user_id=$2")
            .bind(team_id).bind(recipient)
            .fetch_optional(&mut *tx).await.map_err(db_error)?;
        if member.is_some() { return Err(TeamError::Conflict); }
        sqlx::query(
            "UPDATE translation_team_invites
             SET status='expired',decided_at=now()
             WHERE team_id=$1 AND invitee_user_id=$2
               AND status='pending' AND expires_at <= now()")
            .bind(team_id).bind(recipient)
            .execute(&mut *tx).await.map_err(db_error)?;
        sqlx::query(
            "INSERT INTO translation_team_invites
             (token_digest,team_id,invitee_user_id,inviter_user_id)
             VALUES($1,$2,$3,$4)")
            .bind(&hash).bind(team_id).bind(recipient).bind(owner)
            .execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }

    pub async fn detail(&self, recipient: i64, token: &str) -> Result<TeamInvite, TeamError> {
        if recipient <= 0 { return Err(TeamError::Missing); }
        let hash = digest(token)?;
        let row = sqlx::query(
            "SELECT i.team_id,t.name AS team_name,i.invitee_user_id,i.status
             FROM translation_team_invites i
             JOIN translation_teams t ON t.id=i.team_id
             JOIN users owner ON owner.id=t.owner_user_id
             JOIN users recipient ON recipient.id=i.invitee_user_id
             WHERE i.token_digest=$1 AND i.invitee_user_id=$2
               AND i.expires_at>now() AND owner.account_state='active'
               AND recipient.account_state='active'")
            .bind(&hash).bind(recipient)
            .fetch_optional(&self.pool).await.map_err(db_error)?
            .ok_or(TeamError::Missing)?;
        Ok(TeamInvite {
            team_id: row.try_get("team_id").map_err(|_| TeamError::Unavailable)?,
            team_name: row.try_get("team_name").map_err(|_| TeamError::Unavailable)?,
            invitee_user_id: row.try_get("invitee_user_id").map_err(|_| TeamError::Unavailable)?,
            state: row.try_get("status").map_err(|_| TeamError::Unavailable)?,
        })
    }

    /// Concurrent accept/decline serialize on the exact invite row.
    /// Repeated acceptance by the original recipient is idempotent.
    pub async fn decide(&self, actor: i64, token: &str, accept: bool)
        -> Result<i64, TeamError>
    {
        if actor <= 0 { return Err(TeamError::Missing); }
        let hash = digest(token)?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let row = sqlx::query(
            "SELECT i.team_id,i.status,i.expires_at>now() AS live,
                    t.owner_user_id
             FROM translation_team_invites i
             JOIN translation_teams t ON t.id=i.team_id
             JOIN users recipient ON recipient.id=i.invitee_user_id
             JOIN users owner ON owner.id=t.owner_user_id
             WHERE i.token_digest=$1 AND i.invitee_user_id=$2
               AND recipient.account_state='active' AND owner.account_state='active'
             FOR UPDATE OF i")
            .bind(&hash).bind(actor)
            .fetch_optional(&mut *tx).await.map_err(db_error)?
            .ok_or(TeamError::Missing)?;
        let team_id: i64 = row.try_get("team_id").map_err(|_| TeamError::Unavailable)?;
        let status: String = row.try_get("status").map_err(|_| TeamError::Unavailable)?;
        let live: bool = row.try_get("live").map_err(|_| TeamError::Unavailable)?;
        if status == if accept { "accepted" } else { "declined" } {
            tx.commit().await.map_err(db_error)?;
            return Ok(team_id);
        }
        if status != "pending" || !live { return Err(TeamError::Conflict); }
        if accept {
            sqlx::query(
                "INSERT INTO translation_team_members(team_id,user_id,invite_digest)
                 VALUES($1,$2,$3)")
                .bind(team_id).bind(actor).bind(&hash)
                .execute(&mut *tx).await.map_err(db_error)?;
        }
        sqlx::query(
            "UPDATE translation_team_invites
             SET status=$2,decided_at=now() WHERE token_digest=$1")
            .bind(&hash).bind(if accept { "accepted" } else { "declined" })
            .execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(team_id)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_token_inputs() {
        for input in ["", "123", "x/".repeat(22).as_str()] {
            assert_eq!(digest(input), Err(TeamError::Missing));
        }
        assert_eq!(digest(&"a".repeat(43)).unwrap().len(), 32);
    }
}
