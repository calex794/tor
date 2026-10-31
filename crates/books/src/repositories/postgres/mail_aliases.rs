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
                 OR (
                   b.owner_user_id=$1
                   AND EXISTS (
                     SELECT 1 FROM book_members m
                     WHERE m.book_id=b.id AND m.user_id=$1
                       AND m.role='owner' AND m.is_active
                   )
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
}
