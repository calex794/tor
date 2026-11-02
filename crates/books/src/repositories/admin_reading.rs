//! B: administrator-only chapter preview; never a purchase entitlement.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgAdministratorReading {
    pool: PgPool,
}

pub struct AdministratorChapter {
    pub book_title: String,
    pub chapter_title: String,
    pub version_id: i64,
    pub version_no: i32,
    pub access_level: String,
    pub body_markdown: String,
}

impl PgAdministratorReading {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The active A role is checked in the SAME SQL statement as the text.
    /// Explicit version selection is restricted to the requested chapter.
    pub async fn read(
        &self,
        actor: i64,
        chapter_id: i64,
        version_id: Option<i64>,
    ) -> Result<Option<AdministratorChapter>, sqlx::Error> {
        if actor <= 0 || chapter_id <= 0 || version_id.is_some_and(|id| id <= 0) {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title AS book_title, c.title AS chapter_title,
                    c.access_level, v.id AS version_id, v.version_no, v.body_markdown
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_versions v ON v.chapter_id=c.id
             WHERE c.id=$2 AND ($3::BIGINT IS NULL OR v.id=$3)
               AND EXISTS (
                 SELECT 1 FROM users u JOIN account_roles r ON r.user_id=u.id
                 WHERE u.id=$1 AND u.account_state='active'
                   AND r.role='administrator' AND r.is_active
               )
             ORDER BY CASE WHEN v.id = (
                 SELECT p.version_id FROM chapter_publications p
                 WHERE p.chapter_id=c.id
             ) THEN 0 ELSE 1 END, v.version_no DESC, v.id DESC
             LIMIT 1",
        )
        .bind(actor)
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(AdministratorChapter {
            book_title: row.try_get("book_title")?,
            chapter_title: row.try_get("chapter_title")?,
            version_id: row.try_get("version_id")?,
            version_no: row.try_get("version_no")?,
            access_level: row.try_get("access_level")?,
            body_markdown: row.try_get("body_markdown")?,
        }))
    }
}
