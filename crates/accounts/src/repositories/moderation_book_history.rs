//! Site-moderator-only read projection of immutable registration, cover and
//! deletion decisions. No publishing, cover attachment or rights mutation.
use super::{
    moderation_books::{current_moderator, ModerationBooksError},
    PgAccounts,
};
/// Site-only immutable decision record, combining registration, cover
/// moderation and ordinary deletion without exposing identities in Book DTOs.
#[derive(Debug)]
pub struct BookModerationHistoryEntry {
    pub kind: String,
    pub outcome: String,
    pub reviewer_user_id: i64,
    pub reviewer_name: String,
    pub reason: String,
    pub decided_unix: i64,
    pub decision_ref: String,
}

impl PgAccounts {
    /// A read-only audit projection: all three decisions remain owned by
    /// their respective services, while this query never grants publication,
    /// object-store, chapter or purchase rights.
    pub async fn private_book_moderation_history(
        &self,
        session: &[u8; 32],
        book_id: i64,
    ) -> Result<Vec<BookModerationHistoryEntry>, ModerationBooksError> {

        if book_id <= 0 {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        current_moderator(&mut tx, session).await?;
        let rows = sqlx::query(
            "SELECT kind,outcome,reviewer_user_id,reviewer_name,reason,
                    decided_unix,decision_ref
             FROM (
                SELECT 'registration'::text AS kind,
                    d.outcome::text AS outcome,
                    d.reviewer_user_id,
                    COALESCE(u.display_name,'Учётная запись удалена') AS reviewer_name,
                    COALESCE(d.reason,'')::text AS reason,
                    EXTRACT(EPOCH FROM d.decided_at)::bigint AS decided_unix,
                    d.id::text AS decision_ref
                FROM book_registration_decisions d
                LEFT JOIN users u ON u.id=d.reviewer_user_id
                WHERE d.book_id=$1
                UNION ALL
                SELECT 'cover'::text, d.outcome::text, d.reviewer_user_id,
                    COALESCE(u.display_name,'Учётная запись удалена'),
                    COALESCE(d.reason,'')::text,
                    EXTRACT(EPOCH FROM d.decided_at)::bigint,
                    d.request_id::text
                FROM book_cover_review_decisions d
                LEFT JOIN users u ON u.id=d.reviewer_user_id
                WHERE d.book_id=$1
                UNION ALL
                SELECT 'deletion'::text,
                    CASE WHEN d.decision='approve' THEN 'approved' ELSE 'rejected' END,
                    d.moderator_user_id,
                    COALESCE(u.display_name,'Учётная запись удалена'),
                    d.reason::text,
                    EXTRACT(EPOCH FROM d.decided_at)::bigint,
                    d.request_revision::text
                FROM book_ordinary_deletion_decisions d
                LEFT JOIN users u ON u.id=d.moderator_user_id
                WHERE d.book_id=$1
             ) AS decisions
             ORDER BY decided_unix DESC,kind,decision_ref DESC LIMIT 100",
        )
        .bind(book_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        rows.into_iter()
            .map(|r| {
                Ok(BookModerationHistoryEntry {
                    kind: r.try_get("kind").map_err(|_| ModerationBooksError::Unavailable)?,
                    outcome: r.try_get("outcome").map_err(|_| ModerationBooksError::Unavailable)?,
                    reviewer_user_id: r
                        .try_get("reviewer_user_id")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    reviewer_name: r
                        .try_get("reviewer_name")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    reason: r.try_get("reason").map_err(|_| ModerationBooksError::Unavailable)?,
                    decided_unix: r
                        .try_get("decided_unix")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    decision_ref: r
                        .try_get("decision_ref")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                })
            })
            .collect()
    }
}
