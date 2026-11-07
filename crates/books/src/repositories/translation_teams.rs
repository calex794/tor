//! S026/B: owned translation Team is unrelated to Book creator and D grants.
//! Future invites/memberships do not change canonical Book ownership.
#![forbid(unsafe_code)]
use sqlx::{postgres::PgRow, PgPool, Row};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationTeam {
    pub id: i64,
    pub owner_user_id: i64,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamError {
    Invalid,
    Missing,
    Conflict,
    Unavailable,
}

#[derive(Clone)]
pub struct PgTranslationTeams {
    pool: PgPool,
}

pub fn validate_team(name: &str, description: &str) -> Result<(String, String), TeamError> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > 100
        || name.chars().any(char::is_control)
        || description.chars().count() > 2000
        || description.chars().any(|ch| ch.is_control() && ch != '\n')
    {
        return Err(TeamError::Invalid);
    }
    Ok((name.to_owned(), description.trim().to_owned()))
}

fn decode(row: PgRow) -> Result<TranslationTeam, TeamError> {
    Ok(TranslationTeam {
        id: row.try_get("id").map_err(|_| TeamError::Unavailable)?,
        owner_user_id: row.try_get("owner_user_id").map_err(|_| TeamError::Unavailable)?,
        name: row.try_get("name").map_err(|_| TeamError::Unavailable)?,
        description: row.try_get("description").map_err(|_| TeamError::Unavailable)?,
    })
}

fn db_error(error: sqlx::Error) -> TeamError {
    if let sqlx::Error::Database(details) = &error {
        if details.code().as_deref() == Some("23505") {
            return TeamError::Conflict;
        }
    }
    TeamError::Unavailable
}

impl PgTranslationTeams {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Lock the verified active account until the creation transaction ends.
    pub async fn create(
        &self,
        actor: i64,
        name: &str,
        description: &str,
    ) -> Result<TranslationTeam, TeamError> {
        if actor <= 0 {
            return Err(TeamError::Missing);
        }
        let (name, description) = validate_team(name, description)?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if active.is_none() {
            return Err(TeamError::Missing);
        }
        let row = sqlx::query(
            "INSERT INTO translation_teams(owner_user_id,name,description)
             VALUES($1,$2,$3) RETURNING id,owner_user_id,name,description",
        )
        .bind(actor)
        .bind(name)
        .bind(description)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        let team = decode(row)?;
        tx.commit().await.map_err(db_error)?;
        Ok(team)
    }

    pub async fn list_owned(&self, actor: i64) -> Result<Vec<TranslationTeam>, TeamError> {
        if actor <= 0 {
            return Err(TeamError::Missing);
        }
        let rows = sqlx::query(
            "SELECT t.id,t.owner_user_id,t.name,t.description FROM translation_teams t
             JOIN users u ON u.id=t.owner_user_id
             WHERE t.owner_user_id=$1 AND u.account_state='active'
             ORDER BY t.id DESC LIMIT 100",
        )
        .bind(actor)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(decode).collect()
    }

    pub async fn owned(&self, actor: i64, id: i64) -> Result<TranslationTeam, TeamError> {
        if actor <= 0 || id <= 0 {
            return Err(TeamError::Missing);
        }
        let row = sqlx::query(
            "SELECT t.id,t.owner_user_id,t.name,t.description FROM translation_teams t
             JOIN users u ON u.id=t.owner_user_id
             WHERE t.id=$1 AND t.owner_user_id=$2 AND u.account_state='active'",
        )
        .bind(id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?
        .ok_or(TeamError::Missing)?;
        decode(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s026_team_name_is_bounded_and_safe() {
        assert_eq!(validate_team("  Книга  ", " Текст ").unwrap(),
                   ("Книга".into(), "Текст".into()));
        for name in ["", "   ", "\n", "name\u{0000}", "\tT"] {
            assert_eq!(validate_team(name, ""), Err(TeamError::Invalid));
        }
        assert_eq!(validate_team(&"a".repeat(101), ""), Err(TeamError::Invalid));
        assert_eq!(validate_team("Team", &"x".repeat(2001)), Err(TeamError::Invalid));
    }
}
