//! S020: server-verified monotonic Book access-level transitions.
//!
//! HTTP mutations are explicitly disabled by default pending full cross-surface
//! S020/#486 acceptance; the server-side transition is monotonic and scoped.
use super::{book_related::lock_manageable_member, PgAuthoring};
use crate::services::{authoring::AuthorError, owner_transfer_policy::AccessTier};

/// Unknown imported Book tiers are never silently relabeled basic.
/// The result means the transition needs an active pro_adult user grant.
fn transition_requires_grant(current: Option<&str>, requested: &str) -> Result<bool, AuthorError> {
    let target = AccessTier::from_persisted(requested).ok_or(AuthorError::Invalid)?;
    let prior = current
        .and_then(AccessTier::from_persisted)
        .ok_or(AuthorError::Forbidden)?;
    if target < prior {
        return Err(AuthorError::Forbidden);
    }
    Ok(target == AccessTier::ProAdult && prior != AccessTier::ProAdult)
}

pub struct ManagedBookAccessLevel {
    pub title: String,
    // Legacy imported NULL is intentionally read-only and never reclassified.
    pub level: Option<String>,
}

impl PgAuthoring {
    pub async fn manageable_access_level(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<ManagedBookAccessLevel>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT b.title,b.access_level
             FROM books b
             WHERE b.id=$1 AND NOT b.is_deleted AND NOT b.admin_hold
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
               AND EXISTS (
                 SELECT 1 FROM book_members m
                 WHERE m.book_id=b.id AND m.user_id=$2 AND m.is_active
                   AND (m.role IN ('translator','moderator')
                        OR (m.role='owner' AND b.owner_user_id=$2))
               )",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        Ok(row.map(|(title, level)| ManagedBookAccessLevel { title, level }))
    }

    /// Atomic, scoped, monotonic B transition; only a separately gated HTTP
    /// adapter may call this while cross-surface age acceptance is pending.
    pub async fn verified_upgrade_book_access_level(
        &self,
        actor: i64,
        book_id: i64,
        expected: &str,
        requested: &str,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        // Lock Book first, then BookMember: same order as S020 and B-09.
        let stored: Option<Option<String>> = sqlx::query_scalar(
            "SELECT access_level FROM books
             WHERE id=$1 AND NOT is_deleted AND NOT admin_hold
               AND lifecycle_status NOT IN ('pending_deletion','ordinary_deleted') FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let current = stored.ok_or(AuthorError::Missing)?;
        lock_manageable_member(&mut tx, actor, book_id).await?;
        // A client cannot assign a tier to a legacy imported Book whose tier
        // was never attested; do not treat NULL as the weakest tier.
        let current = current.as_deref().ok_or(AuthorError::Forbidden)?;
        if current != expected {
            return Err(AuthorError::Conflict);
        }
        let needs_grant = transition_requires_grant(Some(current), requested)?;
        if needs_grant {
            // Lock the live account and grant until transaction commit, so a
            // concurrent moderation revocation cannot race a 21+ promotion.
            let grant: Option<i64> = sqlx::query_scalar(
                "SELECT g.user_id FROM user_level_grants g
                 JOIN users u ON u.id=g.user_id
                 WHERE g.user_id=$1 AND g.level='pro_adult'
                   AND g.is_active AND u.account_state='active'
                 FOR SHARE OF g, u",
            )
            .bind(actor)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if grant.is_none() {
                return Err(AuthorError::Forbidden);
            }
        }
        if requested != current {
            let changed = sqlx::query(
                "UPDATE books SET access_level=$2
                 WHERE id=$1 AND access_level=$3",
            )
            .bind(book_id)
            .bind(requested)
            .bind(current)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if changed.rows_affected() != 1 {
                return Err(AuthorError::Conflict);
            }
        }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::book_members::PgBookMembers;
    use sqlx::PgPool;
    use tor_contracts::access::BookRole;

    #[test]
    fn s020_access_level_transition_never_downgrades_or_infers_legacy() {
        assert_eq!(transition_requires_grant(Some("basic"), "adult"), Ok(false));
        assert_eq!(
            transition_requires_grant(Some("basic"), "pro_adult"),
            Ok(true)
        );
        assert_eq!(
            transition_requires_grant(Some("adult"), "pro_adult"),
            Ok(true)
        );
        assert_eq!(
            transition_requires_grant(Some("pro_adult"), "pro_adult"),
            Ok(false)
        );
        assert_eq!(transition_requires_grant(Some("adult"), "adult"), Ok(false));
        assert_eq!(
            transition_requires_grant(Some("adult"), "basic"),
            Err(AuthorError::Forbidden)
        );
        assert_eq!(
            transition_requires_grant(None, "basic"),
            Err(AuthorError::Forbidden)
        );
        assert_eq!(
            transition_requires_grant(None, "pro_adult"),
            Err(AuthorError::Forbidden)
        );
        assert_eq!(
            transition_requires_grant(Some("basic"), "not_a_level"),
            Err(AuthorError::Invalid)
        );
    }

    #[tokio::test]
    #[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
    async fn s020_pg_access_level_monotonic_live_grant_membership_and_cas() {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
        assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
        let pool = PgPool::connect(&url).await.unwrap();
        let marker = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        async fn make_user(pool: &PgPool, role: &str, marker: u128) -> i64 {
            sqlx::query_scalar(
                "INSERT INTO users(email_normalized,display_name,password_hash)
                 VALUES($1,'S020 tier','$argon2id$test') RETURNING id",
            )
            .bind(format!("s020-tier-{role}-{marker}@example.test"))
            .fetch_one(pool)
            .await
            .unwrap()
        }
        let owner = make_user(&pool, "owner", marker).await;
        let translator = make_user(&pool, "translator", marker).await;
        let moderator = make_user(&pool, "moderator", marker).await;
        let outsider = make_user(&pool, "outsider", marker).await;
        let book: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                               visibility,access_level,project_status)
             VALUES($1,'ru',$2,$3,'draft','basic','planned') RETURNING id",
        )
        .bind(format!("s020:tier:{marker}"))
        .bind(format!("Тир книги {marker}"))
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
        let unknown: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                               visibility,access_level)
             VALUES($1,'ru',$2,$3,'draft',NULL) RETURNING id",
        )
        .bind(format!("s020:tier:legacy:{marker}"))
        .bind(format!("Legacy tier {marker}"))
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
        let members = PgBookMembers::new(pool.clone());
        members
            .add_collaborator(owner, book, translator, BookRole::Translator)
            .await
            .unwrap();
        members
            .add_collaborator(owner, book, moderator, BookRole::Moderator)
            .await
            .unwrap();
        let repo = PgAuthoring::new(pool.clone());
        assert_eq!(
            repo.verified_upgrade_book_access_level(outsider, book, "basic", "adult")
                .await,
            Err(AuthorError::Missing),
        );
        assert_eq!(
            repo.verified_upgrade_book_access_level(owner, unknown, "basic", "adult")
                .await,
            Err(AuthorError::Forbidden),
        );
        assert_eq!(
            repo.verified_upgrade_book_access_level(translator, book, "basic", "adult")
                .await,
            Ok(()),
        );
        assert_eq!(
            repo.verified_upgrade_book_access_level(owner, book, "basic", "pro_adult")
                .await,
            Err(AuthorError::Conflict),
        );
        assert_eq!(
            repo.verified_upgrade_book_access_level(owner, book, "adult", "basic")
                .await,
            Err(AuthorError::Forbidden),
        );
        assert_eq!(
            repo.verified_upgrade_book_access_level(moderator, book, "adult", "pro_adult")
                .await,
            Err(AuthorError::Forbidden),
        );
        sqlx::query(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES($1,'pro_adult','moderation',TRUE)",
        )
        .bind(moderator)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            repo.verified_upgrade_book_access_level(moderator, book, "adult", "pro_adult")
                .await,
            Ok(()),
        );
        sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
            .bind(moderator)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            repo.verified_upgrade_book_access_level(translator, book, "pro_adult", "adult")
                .await,
            Err(AuthorError::Forbidden),
        );
        let (level, visibility, project): (String, String, String) =
            sqlx::query_as("SELECT access_level,visibility,project_status FROM books WHERE id=$1")
                .bind(book)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (level.as_str(), visibility.as_str(), project.as_str()),
            ("pro_adult", "draft", "planned")
        );
        members
            .revoke_collaborator(owner, book, translator)
            .await
            .unwrap();
        assert_eq!(
            repo.verified_upgrade_book_access_level(translator, book, "pro_adult", "pro_adult")
                .await,
            Err(AuthorError::Missing),
        );
        sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
            .bind(book)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            repo.verified_upgrade_book_access_level(owner, book, "pro_adult", "pro_adult")
                .await,
            Err(AuthorError::Missing),
        );
        // The completed ordinary-deleted Book is a read-only tombstone.
        sqlx::query(
            "UPDATE books SET admin_hold=FALSE, lifecycle_status='ordinary_deleted',
             ordinary_deleted_at=now(), visibility='draft' WHERE id=$1",
        )
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
        assert!(repo
            .manageable_access_level(owner, book)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            repo.verified_upgrade_book_access_level(owner, book, "pro_adult", "pro_adult")
                .await,
            Err(AuthorError::Missing),
        );
        let frozen: String = sqlx::query_scalar("SELECT access_level FROM books WHERE id=$1")
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(frozen, "pro_adult");
        pool.close().await;
    }
}
