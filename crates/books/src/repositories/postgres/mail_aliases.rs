//! Explicit legacy Book aliases for E-08 mail previews.
//! Only user-visible aliases with reviewed one-to-one mappings may resolve.
//! No full hidden catalog enumeration, source_identity guessing or new web route.
use std::collections::HashMap;

use super::PgCatalog;

impl PgCatalog {
    /// Resolve at most the caller-provided bounded alias set under Book ACL.
    /// A private draft requires both canonical owner_user_id and an active
    /// owner membership; held/deleted books never match. Callers still
    /// fetch public/owner metadata through the existing Book repositories.
    pub async fn resolve_visible_mail_aliases(
        &self,
        actor: i64,
        aliases: &[String],
    ) -> Result<HashMap<String, u64>, sqlx::Error> {
        if actor <= 0 || aliases.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT a.alias_key,b.id
             FROM book_legacy_mail_refs a
             JOIN books b ON b.id=a.book_id
             WHERE a.alias_key=ANY($2::text[])
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (
                 b.visibility='public'
                 OR EXISTS (
                   SELECT 1 FROM book_members m
                   WHERE m.book_id=b.id AND m.user_id=$1 AND m.is_active
                     AND m.role IN ('owner','translator','moderator')
                     AND (m.role<>'owner' OR b.owner_user_id=m.user_id)
                 )
               )",
        )
        .bind(actor)
        .bind(aliases)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(alias, id)| u64::try_from(id).ok().map(|id| (alias, id)))
            .collect())
    }

    /// A numeric private Book preview must obey the same live owner-membership
    /// fence as a legacy alias. An owner_user_id alone is not a capability:
    /// after a role change/stale membership no draft title may be projected.
    pub async fn private_mail_title_for_owner(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        sqlx::query_scalar(
            "SELECT b.title FROM books b
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND EXISTS (
                 SELECT 1 FROM book_members m
                 WHERE m.book_id=b.id AND m.user_id=$2
                   AND m.role='owner' AND m.is_active
               )",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
    }

    /// One actor-scoped title read, with no private cover, chapter or write grants.
    pub async fn private_mail_title_for_member(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        sqlx::query_scalar(
            "SELECT b.title FROM books b
             JOIN book_members m ON m.book_id=b.id AND m.user_id=$2
             WHERE b.id=$1 AND m.is_active
               AND m.role IN ('owner','translator','moderator')
               AND (m.role<>'owner' OR b.owner_user_id=m.user_id)
               AND NOT b.admin_hold AND NOT b.is_deleted",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
    }
}
