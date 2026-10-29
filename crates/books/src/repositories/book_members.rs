//! B-09a: PostgreSQL creator ACL for one canonical Book.
//!
//! Membership is not a paid entitlement or a Team role. No HTTP endpoint is
//! installed until the B-09 Layer-lock and owner-transfer gates are available.
use crate::services::authoring::AuthorError;
use sqlx::{PgPool, Postgres, Transaction};
use tor_contracts::access::BookRole;

#[derive(Clone)]
pub struct PgBookMembers {
    pool: PgPool,
}

fn collaborator_role(role: BookRole) -> Result<&'static str, AuthorError> {
    match role {
        BookRole::Translator => Ok("translator"),
        BookRole::Moderator => Ok("moderator"),
        BookRole::Owner => Err(AuthorError::Forbidden),
    }
}

impl PgBookMembers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Book authorization is always scoped to a verified actor and an active
    /// membership. A stale "owner" membership cannot override owner_user_id.
    /// Administrative holds and deleted books are never manageable here.
    pub async fn active_role(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<BookRole>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let value: Option<String> = sqlx::query_scalar(
            "SELECT m.role
             FROM book_members m
             JOIN books b ON b.id=m.book_id
             WHERE m.book_id=$1 AND m.user_id=$2 AND m.is_active
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (m.role<>'owner' OR b.owner_user_id=m.user_id)",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        value
            .map(|role| match role.as_str() {
                "owner" => Ok(BookRole::Owner),
                "translator" => Ok(BookRole::Translator),
                "moderator" => Ok(BookRole::Moderator),
                _ => Err(AuthorError::Unavailable),
            })
            .transpose()
    }

    /// Serializes every membership mutation behind the owner Book row. A
    /// separate transfer boundary must acquire this same lock before changing
    /// owner_user_id; there is no transfer/Layer-bypass endpoint in this slice.
    async fn require_owner(
        tx: &mut Transaction<'_, Postgres>,
        actor: i64,
        book_id: i64,
    ) -> Result<(), AuthorError> {
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND EXISTS (
                 SELECT 1 FROM book_members m
                 WHERE m.book_id=b.id AND m.user_id=$2
                   AND m.is_active AND m.role='owner'
               )
             FOR UPDATE OF b",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        owner.ok_or(AuthorError::Missing)?;
        Ok(())
    }

    /// Owner-only invitation acceptance/storage primitive. The future HTTP
    /// route must verify Layer locks and the target invitation before calling
    /// this. Re-inviting an already active member is a conflict.
    pub async fn add_collaborator(
        &self,
        actor: i64,
        book_id: i64,
        target_user_id: i64,
        role: BookRole,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 || target_user_id <= 0 {
            return Err(AuthorError::Invalid);
        }
        if actor == target_user_id {
            return Err(AuthorError::Forbidden);
        }
        let role = collaborator_role(role)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        Self::require_owner(&mut tx, actor, book_id).await?;
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO book_members(book_id,user_id,role,is_active,share_percent)
             SELECT $1,u.id,$3,TRUE,0 FROM users u WHERE u.id=$2
             ON CONFLICT(book_id,user_id) DO UPDATE
               SET role=EXCLUDED.role,is_active=TRUE,updated_at=now()
             WHERE NOT book_members.is_active
               AND book_members.role<>'owner' AND book_members.share_percent=0
             RETURNING user_id",
        )
        .bind(book_id)
        .bind(target_user_id)
        .bind(role)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        inserted.ok_or(AuthorError::Conflict)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)
    }

    /// Generic role changes cannot grant or revoke ownership.
    pub async fn change_collaborator_role(
        &self,
        actor: i64,
        book_id: i64,
        target_user_id: i64,
        role: BookRole,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 || target_user_id <= 0 {
            return Err(AuthorError::Invalid);
        }
        let role = collaborator_role(role)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        Self::require_owner(&mut tx, actor, book_id).await?;
        let changed: Option<i64> = sqlx::query_scalar(
            "UPDATE book_members SET role=$3,updated_at=now()
             WHERE book_id=$1 AND user_id=$2
               AND is_active AND role<>'owner'
             RETURNING user_id",
        )
        .bind(book_id)
        .bind(target_user_id)
        .bind(role)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        changed.ok_or(AuthorError::Missing)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)
    }

    /// A collaborator with an economic share cannot be removed by changing
    /// staffing alone. Actual revenue shares are separately owned by B-10/D.
    pub async fn revoke_collaborator(
        &self,
        actor: i64,
        book_id: i64,
        target_user_id: i64,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 || target_user_id <= 0 {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        Self::require_owner(&mut tx, actor, book_id).await?;
        let revoked: Option<i64> = sqlx::query_scalar(
            "UPDATE book_members SET is_active=FALSE,updated_at=now()
             WHERE book_id=$1 AND user_id=$2
               AND is_active AND role<>'owner' AND share_percent=0
             RETURNING user_id",
        )
        .bind(book_id)
        .bind(target_user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        revoked.ok_or(AuthorError::Conflict)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)
    }
}
