//! S028: explicitly authorized Team-to-Book association, never a Book ACL.
//! Membership and book ownership are independent boundaries.
#![forbid(unsafe_code)]
use super::translation_teams::TeamError;
use sqlx::{PgPool, Row};

#[derive(Clone, Debug)]
pub struct TeamBookRow {
    pub id: i64,
    pub title: String,
}

#[derive(Clone, Debug)]
pub struct TeamMemberRow {
    pub user_id: i64,
    pub display_name: String,
}

#[derive(Clone, Debug)]
pub struct TeamBooksView {
    pub is_owner: bool,
    pub linked: Vec<TeamBookRow>,
    pub candidates: Vec<TeamBookRow>,
}

#[derive(Clone)]
pub struct PgTeamBooks {
    pool: PgPool,
}

fn db_error(error: sqlx::Error) -> TeamError {
    if let sqlx::Error::Database(ref details) = error {
        if details.code().as_deref() == Some("23505") {
            return TeamError::Conflict;
        }
    }
    TeamError::Unavailable
}

fn decode_book(row: sqlx::postgres::PgRow) -> Result<TeamBookRow, TeamError> {
    Ok(TeamBookRow {
        id: row.try_get("id").map_err(|_| TeamError::Unavailable)?,
        title: row.try_get("title").map_err(|_| TeamError::Unavailable)?,
    })
}

impl PgTeamBooks {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Reads team role and active-account status without granting book access.
    async fn is_team_owner(&self, actor: i64, team_id: i64) -> Result<bool, TeamError> {
        if actor <= 0 || team_id <= 0 {
            return Err(TeamError::Missing);
        }
        sqlx::query_scalar::<_, bool>(
            "SELECT t.owner_user_id=$1 AS is_owner
             FROM translation_teams t
             JOIN users owner ON owner.id=t.owner_user_id AND owner.account_state='active'
             JOIN users actor ON actor.id=$1 AND actor.account_state='active'
             WHERE t.id=$2 AND (
                 t.owner_user_id=$1 OR EXISTS (
                     SELECT 1 FROM translation_team_members m
                     WHERE m.team_id=t.id AND m.user_id=$1
                 )
             )",
        )
        .bind(actor)
        .bind(team_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?
        .ok_or(TeamError::Missing)
    }

    /// Only the team owner or an invited, active member can see linked book titles.
    /// A book is hidden if its owner has ceased to belong to the team.
    pub async fn view(&self, actor: i64, team_id: i64) -> Result<TeamBooksView, TeamError> {
        let is_owner = self.is_team_owner(actor, team_id).await?;
        let linked = sqlx::query(
            "SELECT b.id,b.title FROM translation_team_books tb
             JOIN translation_teams t ON t.id=tb.team_id
             JOIN books b ON b.id=tb.book_id
             WHERE tb.team_id=$1 AND tb.is_active
               AND b.owner_user_id=tb.added_by_user_id
               AND NOT b.is_deleted AND NOT b.admin_hold
               AND (b.owner_user_id=t.owner_user_id OR EXISTS (
                   SELECT 1 FROM translation_team_members member
                   JOIN users u ON u.id=member.user_id AND u.account_state='active'
                   WHERE member.team_id=t.id AND member.user_id=b.owner_user_id
               ))
             ORDER BY tb.created_at DESC,b.id DESC LIMIT 100",
        )
        .bind(team_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(decode_book)
        .collect::<Result<Vec<_>, _>>()?;
        let candidates = sqlx::query(
            "SELECT b.id,b.title FROM books b
             JOIN book_members m ON m.book_id=b.id AND m.user_id=$1
                 AND m.role='owner' AND m.is_active
             WHERE b.owner_user_id=$1 AND NOT b.is_deleted AND NOT b.admin_hold
               AND NOT EXISTS (
                   SELECT 1 FROM translation_team_books tb
                   WHERE tb.team_id=$2 AND tb.book_id=b.id AND tb.is_active
               )
             ORDER BY b.id DESC LIMIT 100",
        )
        .bind(actor)
        .bind(team_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(decode_book)
        .collect::<Result<Vec<_>, _>>()?;
        Ok(TeamBooksView {
            is_owner,
            linked,
            candidates,
        })
    }

    /// Team owner controls the roster; Book roles remain exclusively in B-09.
    pub async fn members(&self, actor: i64, team_id: i64) -> Result<Vec<TeamMemberRow>, TeamError> {
        if !self.is_team_owner(actor, team_id).await? {
            return Err(TeamError::Missing);
        }
        let rows = sqlx::query(
            "SELECT m.user_id,u.display_name
             FROM translation_team_members m
             JOIN users u ON u.id=m.user_id AND u.account_state='active'
             WHERE m.team_id=$1 ORDER BY m.joined_at DESC LIMIT 100",
        )
        .bind(team_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(TeamMemberRow {
                    user_id: row.try_get("user_id").map_err(|_| TeamError::Unavailable)?,
                    display_name: row
                        .try_get("display_name")
                        .map_err(|_| TeamError::Unavailable)?,
                })
            })
            .collect()
    }

    /// The actor must be both a Team member/owner AND the canonical active Book owner.
    /// Lock Team then Book, and never mutate Book owner, collaborator or entitlement tables.
    pub async fn attach(&self, actor: i64, team_id: i64, book_id: i64) -> Result<(), TeamError> {
        if actor <= 0 || team_id <= 0 || book_id <= 0 {
            return Err(TeamError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let team: Option<i64> = sqlx::query_scalar(
            "SELECT t.id FROM translation_teams t
             JOIN users owner ON owner.id=t.owner_user_id AND owner.account_state='active'
             JOIN users actor ON actor.id=$1 AND actor.account_state='active'
             WHERE t.id=$2 AND (t.owner_user_id=$1 OR EXISTS (
                 SELECT 1 FROM translation_team_members m
                 WHERE m.team_id=t.id AND m.user_id=$1
             )) FOR UPDATE OF t",
        )
        .bind(actor)
        .bind(team_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if team.is_none() {
            return Err(TeamError::Missing);
        }
        let owned_book: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b WHERE b.id=$1 AND b.owner_user_id=$2
               AND NOT b.is_deleted AND NOT b.admin_hold
               AND EXISTS (SELECT 1 FROM book_members m WHERE m.book_id=b.id
                   AND m.user_id=$2 AND m.is_active AND m.role='owner')
             FOR UPDATE OF b",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if owned_book.is_none() {
            return Err(TeamError::Missing);
        }
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO translation_team_books(team_id,book_id,added_by_user_id)
             VALUES($1,$2,$3)
             ON CONFLICT(team_id,book_id) DO UPDATE
               SET is_active=TRUE,added_by_user_id=EXCLUDED.added_by_user_id,updated_at=now()
               WHERE NOT translation_team_books.is_active
             RETURNING book_id",
        )
        .bind(team_id)
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        inserted.ok_or(TeamError::Conflict)?;
        tx.commit().await.map_err(db_error)
    }

    /// Removing a book from the Team requires its Team owner, not Book ownership.
    /// No Book membership, sales, chapters or saved-edition data is modified.
    pub async fn detach(&self, actor: i64, team_id: i64, book_id: i64) -> Result<(), TeamError> {
        if actor <= 0 || team_id <= 0 || book_id <= 0 {
            return Err(TeamError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT t.id FROM translation_teams t
             JOIN users u ON u.id=$1 AND u.account_state='active'
             WHERE t.id=$2 AND t.owner_user_id=$1 FOR UPDATE OF t",
        )
        .bind(actor)
        .bind(team_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if owner.is_none() {
            return Err(TeamError::Missing);
        }
        let changed: Option<i64> = sqlx::query_scalar(
            "UPDATE translation_team_books SET is_active=FALSE,updated_at=now()
             WHERE team_id=$1 AND book_id=$2 AND is_active RETURNING book_id",
        )
        .bind(team_id)
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        changed.ok_or(TeamError::Missing)?;
        tx.commit().await.map_err(db_error)
    }

    /// Team owner may remove a member; linked books owned by that member are
    /// unlinked in the same transaction. Their canonical Book ACL is untouched.
    pub async fn remove_member(
        &self,
        actor: i64,
        team_id: i64,
        member_id: i64,
    ) -> Result<(), TeamError> {
        if actor <= 0 || team_id <= 0 || member_id <= 0 || actor == member_id {
            return Err(TeamError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT t.id FROM translation_teams t
             JOIN users u ON u.id=$1 AND u.account_state='active'
             WHERE t.id=$2 AND t.owner_user_id=$1 FOR UPDATE OF t",
        )
        .bind(actor)
        .bind(team_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if owner.is_none() {
            return Err(TeamError::Missing);
        }
        let removed: Option<i64> = sqlx::query_scalar(
            "DELETE FROM translation_team_members
             WHERE team_id=$1 AND user_id=$2 RETURNING user_id",
        )
        .bind(team_id)
        .bind(member_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        removed.ok_or(TeamError::Missing)?;
        sqlx::query(
            "UPDATE translation_team_books tb SET is_active=FALSE,updated_at=now()
             FROM books b WHERE tb.team_id=$1 AND b.id=tb.book_id
               AND b.owner_user_id=$2 AND tb.is_active",
        )
        .bind(team_id)
        .bind(member_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }
}
