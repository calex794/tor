//! SQL-level public book and chapter visibility; raw versions are never returned by ID.
use crate::services::{
    PublicBookPage, PublicChapterLink, PublicChapterPage, PublicReadingRepository, ReadingError,
};
use async_trait::async_trait;
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::time::Duration;

#[derive(Clone)]
pub struct PgReader {
    pool: PgPool,
}

impl PgReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        // Catalog (3 connections) + reader (2) fit tor_app's 6-connection cap.
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await?;
        let role: String = sqlx::query_scalar("SELECT current_user")
            .fetch_one(&pool)
            .await?;
        if role != "tor_app" {
            return Err(sqlx::Error::Protocol(
                "Public reader must use the restricted tor_app role".into(),
            ));
        }
        let reader = Self::new(pool);
        reader.probe().await?;
        Ok(reader)
    }

    pub async fn probe(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "SELECT c.id, v.body_markdown FROM chapters c \
             JOIN chapter_publications p ON p.chapter_id = c.id \
             JOIN chapter_versions v ON v.id = p.version_id AND v.chapter_id = c.id \
             JOIN books b ON b.id = c.book_id \
             WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted \
             AND c.visibility='public' AND c.access_level='free' \
             AND NOT c.admin_hold AND NOT c.is_deleted LIMIT 0",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn map_db(_: sqlx::Error) -> ReadingError {
    ReadingError::Unavailable
}

fn positive_id(value: i64) -> Result<u64, ReadingError> {
    u64::try_from(value)
        .ok()
        .filter(|id| *id > 0)
        .ok_or(ReadingError::Unavailable)
}

#[async_trait]
impl PublicReadingRepository for PgReader {
    async fn public_book(&self, id: u64) -> Result<Option<PublicBookPage>, ReadingError> {
        let Ok(id) = i64::try_from(id) else {
            return Ok(None);
        };
        if id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT id, title, target_language FROM books \
             WHERE id = $1 AND visibility = 'public' AND NOT admin_hold AND NOT is_deleted",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db)?;
        let Some(book) = row else {
            return Ok(None);
        };
        // Recheck the parent state in the chapter query, not just in the first query.
        let rows = sqlx::query(
            "SELECT c.id, c.chapter_no, c.title, c.access_level, c.price_amount FROM chapters AS c \
             JOIN books AS b ON b.id = c.book_id \
             JOIN chapter_publications AS p ON p.chapter_id = c.id \
             JOIN chapter_versions AS v ON v.id = p.version_id AND v.chapter_id = c.id \
             WHERE c.book_id = $1 AND b.visibility = 'public' AND NOT b.admin_hold \
               AND NOT b.is_deleted AND c.visibility = 'public' \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now()) \
             ORDER BY c.chapter_no, c.id LIMIT 200",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_db)?;
        let chapters = rows
            .into_iter()
            .map(|row| {
                Ok(PublicChapterLink {
                    id: positive_id(row.try_get("id").map_err(map_db)?)?,
                    number: row.try_get("chapter_no").map_err(map_db)?,
                    title: row.try_get("title").map_err(map_db)?,
                    is_paid: row.try_get::<String, _>("access_level").map_err(map_db)? == "paid",
                    price_minor: row.try_get("price_amount").map_err(map_db)?,
                })
            })
            .collect::<Result<Vec<_>, ReadingError>>()?;
        Ok(Some(PublicBookPage {
            id: positive_id(book.try_get("id").map_err(map_db)?)?,
            title: book.try_get("title").map_err(map_db)?,
            language: book.try_get("target_language").map_err(map_db)?,
            chapters,
        }))
    }

    async fn public_chapter(&self, id: u64) -> Result<Option<PublicChapterPage>, ReadingError> {
        let Ok(id) = i64::try_from(id) else {
            return Ok(None);
        };
        if id <= 0 {
            return Ok(None);
        }
        // Access predicates must be in the SAME SQL query that selects the text.
        // Never rely on filtering navigation links, the homepage or an earlier request.
        let row = sqlx::query(
            "SELECT b.id AS book_id, b.title AS book_title, \
                    c.title AS chapter_title, p.version_id, v.version_no, v.body_markdown, \
                    (SELECT prior.id FROM chapters prior \
                     JOIN chapter_publications pp ON pp.chapter_id = prior.id \
                     JOIN chapter_versions pv ON pv.id = pp.version_id AND pv.chapter_id = prior.id \
                     WHERE prior.book_id = c.book_id AND prior.chapter_no < c.chapter_no \
                       AND prior.visibility = 'public' AND prior.access_level = 'free' \
                       AND NOT prior.admin_hold AND NOT prior.is_deleted \
                       AND (prior.reader_released_at IS NULL OR prior.reader_released_at <= now()) \
                     ORDER BY prior.chapter_no DESC LIMIT 1) AS previous_chapter_id, \
                    (SELECT following.id FROM chapters following \
                     JOIN chapter_publications fp ON fp.chapter_id = following.id \
                     JOIN chapter_versions fv ON fv.id = fp.version_id AND fv.chapter_id = following.id \
                     WHERE following.book_id = c.book_id AND following.chapter_no > c.chapter_no \
                       AND following.visibility = 'public' AND following.access_level = 'free' \
                       AND NOT following.admin_hold AND NOT following.is_deleted \
                       AND (following.reader_released_at IS NULL OR following.reader_released_at <= now()) \
                     ORDER BY following.chapter_no ASC LIMIT 1) AS next_chapter_id \
             FROM chapters AS c \
             JOIN books AS b ON b.id = c.book_id \
             JOIN chapter_publications AS p ON p.chapter_id = c.id \
             JOIN chapter_versions AS v ON v.id = p.version_id AND v.chapter_id = c.id \
             WHERE c.id = $1 AND b.visibility = 'public' AND NOT b.admin_hold \
               AND NOT b.is_deleted AND c.visibility = 'public' AND c.access_level = 'free' \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now())",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db)?;
        row.map(|row| {
            Ok(PublicChapterPage {
                version_id: positive_id(row.try_get("version_id").map_err(map_db)?)?,
                book_id: positive_id(row.try_get("book_id").map_err(map_db)?)?,
                book_title: row.try_get("book_title").map_err(map_db)?,
                chapter_title: row.try_get("chapter_title").map_err(map_db)?,
                version_no: row.try_get("version_no").map_err(map_db)?,
                body_markdown: row.try_get("body_markdown").map_err(map_db)?,
                previous_chapter_id: row
                    .try_get::<Option<i64>, _>("previous_chapter_id")
                    .map_err(map_db)?
                    .map(positive_id)
                    .transpose()?,
                next_chapter_id: row
                    .try_get::<Option<i64>, _>("next_chapter_id")
                    .map_err(map_db)?
                    .map(positive_id)
                    .transpose()?,
            })
        })
        .transpose()
    }
}
