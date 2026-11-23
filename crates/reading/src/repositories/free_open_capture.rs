//! C-08 / D-001: save exact reader-opened FREE identities without
//! granting rights to unvisited chapters or rewriting already pinned V1.
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgFreeOpenCapture {
    pool: PgPool,
}

impl PgFreeOpenCapture {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Records a version selected by the trusted reader or a browser-confirmed,
    /// hash-checked S3 read. The browser cannot choose arbitrary old editions.
    /// The *first* opened exact edition wins; repeats never silently upgrade it.
    pub async fn record(
        &self,
        actor: i64,
        chapter: i64,
        edition: i64,
    ) -> Result<bool, sqlx::Error> {
        if actor <= 0 || chapter <= 0 || edition <= 0 {
            return Ok(false);
        }
        let book: Option<i64> = sqlx::query_scalar(
            "SELECT book_id FROM chapters WHERE id=$1",
        )
        .bind(chapter)
        .fetch_optional(&self.pool)
        .await?;
        let Some(book) = book else {
            return Ok(false);
        };
        let mut tx = self.pool.begin().await?;
        lock_actor_book(&mut tx, actor, book).await?;
        let eligible: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN users u ON u.id=$1 AND u.account_state='active'
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=$3
             WHERE c.id=$2 AND b.id=$4
               AND c.visibility='public' AND c.access_level='free'
               AND c.price_amount=0 AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND b.visibility_status='active'
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.access_level='basic'
                    OR (b.access_level='adult' AND u.adult_enabled)
                    OR (b.access_level='pro_adult' AND EXISTS (
                        SELECT 1 FROM user_level_grants g
                        WHERE g.user_id=u.id AND g.level='pro_adult'
                          AND g.is_active)))
             FOR SHARE OF b,c,u,v",
        )
        .bind(actor)
        .bind(chapter)
        .bind(edition)
        .bind(book)
        .fetch_optional(&mut *tx)
        .await?;
        if eligible.is_none() {
            tx.commit().await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO reader_free_opened_editions(user_id,chapter_id,version_id)
             VALUES($1,$2,$3)
             ON CONFLICT(user_id,chapter_id) DO NOTHING",
        )
        .bind(actor)
        .bind(chapter)
        .bind(edition)
        .execute(&mut *tx)
        .await?;
        // A Book previously bookmarked by the same actor automatically retains
        // only this legitimately opened chapter, with its first open version.
        // The common actor+Book advisory lock serializes this with add/remove.
        sqlx::query(
            "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
             SELECT opened.user_id,opened.chapter_id,opened.version_id
             FROM reader_free_opened_editions opened
             JOIN reader_library_items saved
               ON saved.user_id=opened.user_id AND saved.book_id=$4
             WHERE opened.user_id=$1 AND opened.chapter_id=$2
             ON CONFLICT(user_id,chapter_id) DO NOTHING",
        )
        .bind(actor)
        .bind(chapter)
        .bind(edition)
        .bind(book)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}

/// Serialize Book add/remove and verified FREE open for the same actor/Book,
/// without locking every reader of a popular Book. Always acquire before
/// Book row SHARE locks and bookmark mutation in these three paths.
pub(crate) async fn lock_actor_book(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(
            hashtextextended('c08-free-open:' || $1::text || ':' || $2::text,0))",
    )
    .bind(actor)
    .bind(book)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
