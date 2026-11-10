//! Preserve the existing bookmark/free-pin statement and capture only new inserts.
use super::{LibraryError, PgLibrary};

impl PgLibrary {
    pub async fn add_book(&self, user_id: i64, book_id: i64) -> Result<(), LibraryError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(LibraryError::Invalid);
        }
        // One SQL statement atomically saves the Book link AND every currently
        // released free edition, exactly as NEwBOO's add-to-library action.
        // Pin by immutable version ID. Existing pins are never upgraded to V2
        // and paid/unreleased/draft/held chapters are never granted.
        // Lock the Book row to serialize a simultaneous deletion decision.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LibraryError::Unavailable)?;
        super::super::flow_capture::prepare_action(&mut tx, self.flow_collection)
            .await
            .map_err(|_| LibraryError::Unavailable)?;
        let saved: Option<i64> = sqlx::query_scalar(
            "WITH eligible AS MATERIALIZED ( \
                SELECT b.id FROM books AS b \
                JOIN users AS u ON u.id=$1 AND u.account_state='active' \
                WHERE b.id=$2 AND b.visibility='public' \
                  AND b.visibility_status='active' \
                  AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted') \
                  AND NOT b.admin_hold AND NOT b.is_deleted \
                  AND (b.access_level='basic' \
                       OR (b.access_level='adult' AND u.adult_enabled) \
                       OR (b.access_level='pro_adult' AND EXISTS ( \
                           SELECT 1 FROM user_level_grants AS g \
                           WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active \
                       ))) \
                FOR SHARE OF b \
             ), saved_book AS ( \
                INSERT INTO reader_library_items (user_id, book_id) \
                SELECT $1, e.id FROM eligible AS e \
                ON CONFLICT (user_id, book_id) DO UPDATE SET book_id=EXCLUDED.book_id \
                RETURNING book_id \
             ), saved_free AS ( \
                INSERT INTO reader_saved_free_chapters (user_id, chapter_id, version_id) \
                SELECT $1, c.id, p.version_id \
                FROM saved_book AS li \
                JOIN chapters AS c ON c.book_id=li.book_id \
                JOIN chapter_publications AS p ON p.chapter_id=c.id \
                JOIN chapter_versions AS v ON v.chapter_id=c.id AND v.id=p.version_id \
                WHERE c.visibility='public' AND c.access_level='free' \
                  AND NOT c.admin_hold AND NOT c.is_deleted \
                  AND (c.reader_released_at IS NULL OR c.reader_released_at<=now()) \
                ON CONFLICT (user_id, chapter_id) DO NOTHING \
                RETURNING chapter_id \
             ) \
             SELECT li.book_id FROM saved_book AS li \
             CROSS JOIN (SELECT COUNT(*) FROM saved_free) AS pins",
        )
        .bind(user_id)
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LibraryError::Unavailable)?;
        let failed = super::super::flow_capture::action_failed(&mut tx)
            .await
            .map_err(|_| LibraryError::Unavailable)?;
        tx.commit().await.map_err(|_| LibraryError::Unavailable)?;
        super::super::flow_capture::action_committed(failed);
        saved.map(|_| ()).ok_or(LibraryError::NotFound)
    }
}
