//! Account-owned settings, serialized per user and protected by revision CAS.
use super::PgAccounts;
use crate::services::profile::ProfileUpdate;

pub struct Profile {
    pub user_id: i64,
    pub display_name: String,
    pub handle: Option<String>,
    pub adult_enabled: bool,
    pub revision: i64,
}

#[derive(Debug)]
pub enum ProfileError {
    Stale,
    Duplicate,
    Unavailable,
}
impl From<sqlx::Error> for ProfileError {
    fn from(error: sqlx::Error) -> Self {
        if matches!(&error, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505")) {
            Self::Duplicate
        } else {
            Self::Unavailable
        }
    }
}

impl PgAccounts {
    /// NEwBOO display-name URLs use trimmed, case-insensitive keys.
    /// PostgreSQL currently permits collisions, so any two folded matches
    /// must deny resolution rather than choose an arbitrary recipient.
    /// Never return email, credentials, access tier or a private profile record.
    pub async fn public_profile_by_display_name(
        &self,
        display_name: &str,
    ) -> Result<Option<(i64, String)>, sqlx::Error> {
        let normalized = display_name.trim();
        if normalized.is_empty() || normalized.chars().count() > 80 {
            return Ok(None);
        }
        let mut rows = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, display_name FROM users
             WHERE lower(display_name)=lower($1::text)
             ORDER BY id LIMIT 2",
        )
        .bind(normalized)
        .fetch_all(self.pool())
        .await?;
        Ok(if rows.len() == 1 { rows.pop() } else { None })
    }

    pub async fn profile(&self, user_id: i64) -> Result<Option<Profile>, sqlx::Error> {
        let row: Option<(i64, String, Option<String>, bool, i64)> = sqlx::query_as(
            "SELECT u.id,u.display_name,s.handle,u.adult_enabled,COALESCE(s.revision,0)
             FROM users u LEFT JOIN account_settings s ON s.user_id=u.id WHERE u.id=$1",
        )
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|r| Profile {
            user_id: r.0,
            display_name: r.1,
            handle: r.2,
            adult_enabled: r.3,
            revision: r.4,
        }))
    }

    /// The actor comes from the live session; there is no target-user argument.
    pub async fn update_profile(
        &self,
        actor: i64,
        update: &ProfileUpdate,
    ) -> Result<i64, ProfileError> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(actor)
            .fetch_one(&mut *tx)
            .await?;
        let revision: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM account_settings WHERE user_id=$1")
                .bind(actor)
                .fetch_optional(&mut *tx)
                .await?;
        if revision.unwrap_or(0) != update.expected_revision {
            return Err(ProfileError::Stale);
        }
        sqlx::query("UPDATE users SET display_name=COALESCE($2,display_name),adult_enabled=COALESCE($3,adult_enabled) WHERE id=$1")
            .bind(actor).bind(&update.display_name).bind(update.adult_enabled).execute(&mut *tx).await?;
        let next: i64 = sqlx::query_scalar(
            "INSERT INTO account_settings(user_id,handle) VALUES($1,NULLIF($2,''))
             ON CONFLICT(user_id) DO UPDATE SET
               handle=CASE WHEN $2::text IS NULL THEN account_settings.handle ELSE NULLIF($2,'') END,
               revision=account_settings.revision+1,updated_at=now() RETURNING revision",
        ).bind(actor).bind(&update.handle).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(next)
    }
}
