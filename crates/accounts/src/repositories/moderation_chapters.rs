//! A-05/MOD-C01: bounded chapter metadata queue and reversible moderator hold.
//! Author publishing, paid ownership and immutable versions are separate.
use super::{
    moderation_books::{current_moderator, ModerationBooksError},
    PgAccounts,
};

#[derive(Debug, sqlx::FromRow)]
pub struct ModerationChapter {
    pub id: i64,
    pub book_id: i64,
    pub book_title: String,
    pub chapter_no: i32,
    pub title: String,
    pub visibility: String,
    pub access_level: String,
    pub admin_hold: bool,
    pub moderation_state: String,
    pub has_publication: bool,
    pub has_version: bool,
    pub edit_revision: i64,
    pub current_version_id: Option<i64>,
    pub last_decision_id: i64,
}

/// Immutable expectation captured by a moderator's publish/draft form.
pub struct ChapterPublicationCommand<'a> {
    pub expected_moderation_state: &'a str,
    pub expected_visibility: &'a str,
    pub expected_revision: i64,
    pub expected_version: Option<i64>,
    pub expected_decision_id: i64,
    pub action: &'a str,
}

#[derive(Debug, sqlx::FromRow)]
struct ChapterPublicationRow {
    visibility: String,
    moderation_state: String,
    admin_hold: bool,
    edit_revision: i64,
    access_level: String,
    price_amount: i64,
    version_id: Option<i64>,
    working_version_id: Option<i64>,
    release_due: bool,
}

impl PgAccounts {
    /// A moderator sees metadata, never body_markdown or a buyer grant.
    /// Session, live role and user remain locked until the read commits.
    pub async fn moderation_chapters(
        &self,
        session: &[u8; 32],
        before: Option<i64>,
        status: &str,
    ) -> Result<Vec<ModerationChapter>, ModerationBooksError> {
        if before.is_some_and(|value| value <= 0)
            || !matches!(
                status,
                "all" | "hold" | "hidden" | "blocked" | "draft" | "published" | "no_version"
            )
        {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        current_moderator(&mut tx, session).await?;
        let rows: Vec<ModerationChapter> =
            sqlx::query_as(
                "SELECT c.id, c.book_id, b.title AS book_title, c.chapter_no, c.title,
                        c.visibility, c.access_level, c.admin_hold, c.moderation_state,
                        (p.chapter_id IS NOT NULL) AS has_publication,
                        EXISTS (SELECT 1 FROM chapter_versions v WHERE v.chapter_id=c.id)
                            AS has_version,
                        c.edit_revision,COALESCE(p.version_id,d.version_id) AS current_version_id,
                        (SELECT COALESCE(MAX(e.id),0) FROM moderation_chapter_decision_events e
                         WHERE e.chapter_id=c.id) AS last_decision_id
                 FROM chapters c
                 JOIN books b ON b.id=c.book_id
                 LEFT JOIN chapter_publications p ON p.chapter_id=c.id
                 LEFT JOIN chapter_working_drafts d ON d.chapter_id=c.id
                 WHERE NOT c.is_deleted AND NOT b.is_deleted
                   AND ($1::bigint IS NULL OR c.id < $1)
                   AND ($2='all'
                        OR ($2='hold' AND c.admin_hold)
                        OR ($2='hidden' AND c.moderation_state='hidden')
                        OR ($2='blocked' AND c.moderation_state='blocked')
                        OR ($2='draft' AND c.visibility='draft')
                        OR ($2='published' AND c.visibility='public' AND p.chapter_id IS NOT NULL)
                        OR ($2='no_version'
                            AND NOT EXISTS (SELECT 1 FROM chapter_versions v WHERE v.chapter_id=c.id)))
                 ORDER BY c.id DESC LIMIT 40",
            )
            .bind(before)
            .bind(status)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(rows)
    }

    /// Legacy boolean requests are generic holds only: a stale form must not
    /// accidentally clear a distinct moderator hidden/blocked decision.
    /// Both entry points now use the same CAS and append-only decision audit.
    pub async fn moderate_chapter_hold(
        &self,
        session: &[u8; 32],
        chapter_id: i64,
        expected_hold: bool,
        new_hold: bool,
    ) -> Result<(), ModerationBooksError> {
        let expected = if expected_hold { "hold" } else { "none" };
        let next = if new_hold { "hold" } else { "none" };
        self.moderate_chapter_decision(session, chapter_id, expected, next)
            .await
    }

    /// Preserve NEwBOO's distinct hide/block decisions without altering an
    /// author-owned draft/public state, a published version or paid rights.
    /// A changed decision is always audited, even if both states are held.
    pub async fn moderate_chapter_decision(
        &self,
        session: &[u8; 32],
        chapter_id: i64,
        expected_state: &str,
        next_state: &str,
    ) -> Result<(), ModerationBooksError> {
        fn allowed(value: &str) -> bool {
            matches!(value, "none" | "hold" | "hidden" | "blocked")
        }
        if chapter_id <= 0
            || expected_state == next_state
            || !allowed(expected_state)
            || !allowed(next_state)
        {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let actor = current_moderator(&mut tx, session).await?;
        let previous: Option<(bool, String, i64)> = sqlx::query_as(
            "SELECT c.admin_hold,c.moderation_state,c.book_id
             FROM chapters c JOIN books b ON b.id=c.book_id
             WHERE c.id=$1 AND NOT c.is_deleted AND NOT b.is_deleted
             FOR UPDATE OF c",
        )
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        let Some((previous_hold, previous_state, book_id)) = previous else {
            return Err(ModerationBooksError::Missing);
        };
        if previous_state != expected_state || previous_hold != (expected_state != "none") {
            return Err(ModerationBooksError::Stale);
        }
        let next_hold = next_state != "none";
        sqlx::query("UPDATE chapters SET admin_hold=$2, moderation_state=$3 WHERE id=$1")
            .bind(chapter_id)
            .bind(next_hold)
            .bind(next_state)
            .execute(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        sqlx::query(
            "INSERT INTO moderation_chapter_decision_events
                (actor_user_id,book_id,chapter_id,before_state,after_state)
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(actor)
        .bind(book_id)
        .bind(chapter_id)
        .bind(&previous_state)
        .bind(next_state)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        if previous_hold != next_hold {
            sqlx::query(
                "INSERT INTO moderation_chapter_hold_events
                    (actor_user_id,book_id,chapter_id,before_hold,after_hold)
                 VALUES($1,$2,$3,$4,$5)",
            )
            .bind(actor)
            .bind(book_id)
            .bind(chapter_id)
            .bind(previous_hold)
            .bind(next_hold)
            .execute(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        }
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(())
    }
    /// NEwBOO moderator publish/draft: publication state is separate from
    /// moderation holds. Never substitute a moderator for the book owner.
    pub async fn moderate_chapter_publication(
        &self,
        session: &[u8; 32],
        chapter_id: i64,
        command: ChapterPublicationCommand<'_>,
    ) -> Result<(), ModerationBooksError> {
        let ChapterPublicationCommand {
            expected_moderation_state,
            expected_visibility,
            expected_revision,
            expected_version,
            expected_decision_id,
            action,
        } = command;
        if chapter_id <= 0
            || expected_revision < 0
            || expected_decision_id < 0
            || expected_version.is_some_and(|id| id <= 0)
            || !matches!(
                expected_moderation_state,
                "none" | "hold" | "hidden" | "blocked"
            )
            || !matches!(expected_visibility, "draft" | "public")
            || !matches!(action, "published" | "draft")
        {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let actor = current_moderator(&mut tx, session).await?;
        // Same lock ordering as B authoring: book before chapter.
        let book_id: Option<i64> =
            sqlx::query_scalar("SELECT book_id FROM chapters WHERE id=$1 AND NOT is_deleted")
                .bind(chapter_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| ModerationBooksError::Unavailable)?;
        let book_id = book_id.ok_or(ModerationBooksError::Missing)?;
        let book: Option<(String, String, bool, Option<i64>)> = sqlx::query_as(
            "SELECT visibility,visibility_status,admin_hold,owner_user_id
             FROM books WHERE id=$1 AND NOT is_deleted FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        let (book_visibility, book_status, book_hold, owner) =
            book.ok_or(ModerationBooksError::Missing)?;
        let row: Option<ChapterPublicationRow> = sqlx::query_as(
            "SELECT c.visibility,c.moderation_state,c.admin_hold,c.edit_revision,
                    c.access_level,c.price_amount,p.version_id,
                    d.version_id AS working_version_id,
                    (c.reader_released_at IS NULL OR c.reader_released_at<=now()) AS release_due
             FROM chapters c
             LEFT JOIN chapter_publications p ON p.chapter_id=c.id
             LEFT JOIN chapter_working_drafts d ON d.chapter_id=c.id
             WHERE c.id=$1 AND c.book_id=$2 AND NOT c.is_deleted
             FOR UPDATE OF c",
        )
        .bind(chapter_id)
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        let ChapterPublicationRow {
            visibility,
            moderation_state: mod_state,
            admin_hold: held,
            edit_revision: revision,
            access_level: access,
            price_amount: price,
            version_id: publication,
            working_version_id: working,
            release_due,
        } = row.ok_or(ModerationBooksError::Missing)?;
        // The append-only audit sequence also fences ABA: an old publish
        // form cannot republish after someone else published and drafted it.
        let last_decision_id: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(id),0) FROM moderation_chapter_decision_events WHERE chapter_id=$1",
        )
        .bind(chapter_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        if last_decision_id != expected_decision_id {
            return Err(ModerationBooksError::Stale);
        }
        if visibility != expected_visibility
            || mod_state != expected_moderation_state
            || held != (mod_state != "none")
            || revision != expected_revision
            || publication.or(working) != expected_version
        {
            return Err(ModerationBooksError::Stale);
        }
        let before = if mod_state != "none" {
            mod_state.as_str()
        } else if visibility == "public" {
            "published"
        } else {
            "draft"
        };
        if before == action {
            return Err(ModerationBooksError::Stale);
        }
        if action == "published" {
            if !release_due {
                return Err(ModerationBooksError::Stale);
            }
            if publication.is_none() {
                // New paid chapters require B/G's verified object attachment.
                // Only a current FREE working draft can be first-released here.
                if access != "free"
                    || price != 0
                    || book_visibility != "public"
                    || book_status != "active"
                    || book_hold
                {
                    return Err(ModerationBooksError::Stale);
                }
                let version = working.ok_or(ModerationBooksError::Stale)?;
                let inserted = sqlx::query(
                    "INSERT INTO chapter_publications(chapter_id,version_id)
                     VALUES($1,$2) ON CONFLICT(chapter_id) DO NOTHING",
                )
                .bind(chapter_id)
                .bind(version)
                .execute(&mut *tx)
                .await
                .map_err(|_| ModerationBooksError::Unavailable)?;
                if inserted.rows_affected() != 1 {
                    return Err(ModerationBooksError::Stale);
                }
                sqlx::query("DELETE FROM chapter_working_drafts WHERE chapter_id=$1")
                    .bind(chapter_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| ModerationBooksError::Unavailable)?;
                let release = sqlx::query(
                    "INSERT INTO chapter_first_release_events
                       (chapter_id,version_id,creator_user_id)
                     VALUES($1,$2,$3) ON CONFLICT(chapter_id) DO NOTHING",
                )
                .bind(chapter_id)
                .bind(version)
                .bind(owner)
                .execute(&mut *tx)
                .await
                .map_err(|_| ModerationBooksError::Unavailable)?;
                if release.rows_affected() != 1 {
                    return Err(ModerationBooksError::Stale);
                }
            } else if (access == "paid" && price <= 0) || (access == "free" && price != 0) {
                return Err(ModerationBooksError::Stale);
            }
            // Existing paid pointer is B-owned: never attach another version,
            // edit an encrypted object or change a purchase grant here.
        }
        let next_visibility = if action == "published" {
            "public"
        } else {
            "draft"
        };
        sqlx::query(
            "UPDATE chapters SET visibility=$2,admin_hold=false,
                moderation_state='none' WHERE id=$1",
        )
        .bind(chapter_id)
        .bind(next_visibility)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        sqlx::query(
            "INSERT INTO moderation_chapter_decision_events
                (actor_user_id,book_id,chapter_id,before_state,after_state)
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(actor)
        .bind(book_id)
        .bind(chapter_id)
        .bind(before)
        .bind(action)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        if held {
            sqlx::query(
                "INSERT INTO moderation_chapter_hold_events
                    (actor_user_id,book_id,chapter_id,before_hold,after_hold)
                 VALUES($1,$2,$3,true,false)",
            )
            .bind(actor)
            .bind(book_id)
            .bind(chapter_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        }
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(())
    }
}
