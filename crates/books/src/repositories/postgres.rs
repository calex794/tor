//! Public catalog SQLx adapter, ported from bokarust@d11b27b.
mod book_description;
mod chapter_edit;
mod chapter_outbox;
mod chapter_publish_free;
mod chapter_published_edit;
mod chapter_release;
mod owned_book_hash;
use crate::{
    domain::{Book, BookKey, Visibility},
    services::{
        authoring::{
            valid_command_key, AuthorError, ChapterAccess, NewBook, OwnedBook, SaveChapter,
            SavedChapter,
        },
        CatalogError, PublicBookRepository, MAX_PAGE_SIZE,
    },
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    PgPool, Row,
};
use std::time::Duration;
mod readiness;
use readiness::{require_latest_migration, REQUIRED_MIGRATION};

#[derive(Clone)]
pub struct PgCatalog {
    pool: PgPool,
}
impl PgCatalog {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    /// Share one bounded pool with the reader and accounts; no extra connections.
    pub fn pool(&self) -> PgPool {
        self.pool.clone()
    }
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await?;
        let role: String = sqlx::query_scalar("SELECT current_user")
            .fetch_one(&pool)
            .await?;
        if role != "tor_app" {
            return Err(sqlx::Error::Protocol(
                "TOR runtime requires the tor_app database role".into(),
            ));
        }
        let catalog = Self::new(pool);
        catalog.probe().await?;
        Ok(catalog)
    }
    /// Explicit pool shutdown; used to verify loss-of-readiness behavior.
    pub async fn close_for_test(&self) {
        self.pool.close().await;
    }
    /// Unavailable schema, missing migration, and broken PG return 503 through web.
    pub async fn probe(&self) -> Result<(), sqlx::Error> {
        let applied: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations WHERE version=$1 AND success=TRUE",
        )
        .bind(REQUIRED_MIGRATION)
        .fetch_optional(&self.pool)
        .await?;
        require_latest_migration(applied)?;
        sqlx::query(
            "SELECT id, source_identity, target_language, title, visibility \
             FROM books WHERE visibility='public' AND NOT admin_hold \
             AND NOT is_deleted LIMIT 0",
        )
        .execute(&self.pool)
        .await?;
        // A stale schema can still serve catalog pages while B-06 chapter
        // writes fail. Do not report readiness until its working table is
        // present and readable by the restricted application role.
        sqlx::query("SELECT chapter_id, version_id FROM chapter_working_drafts LIMIT 0")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
fn decode_public(row: PgRow) -> Result<Book, CatalogError> {
    let id: i64 = row.try_get("id").map_err(|_| CatalogError::Unavailable)?;
    let identity: String = row
        .try_get("source_identity")
        .map_err(|_| CatalogError::Unavailable)?;
    let language: String = row
        .try_get("target_language")
        .map_err(|_| CatalogError::Unavailable)?;
    let title: String = row
        .try_get("title")
        .map_err(|_| CatalogError::Unavailable)?;
    let visibility: String = row
        .try_get("visibility")
        .map_err(|_| CatalogError::Unavailable)?;
    if visibility != "public" {
        return Err(CatalogError::Unavailable);
    }
    let id = u64::try_from(id).map_err(|_| CatalogError::Unavailable)?;
    let key = BookKey::new(identity, language).map_err(|_| CatalogError::Unavailable)?;
    Book::new(id, key, title, Visibility::Public).map_err(|_| CatalogError::Unavailable)
}
#[async_trait]
impl PublicBookRepository for PgCatalog {
    async fn list_public(&self, limit: usize) -> Result<Vec<Book>, CatalogError> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(CatalogError::InvalidLimit);
        }
        let limit = i64::try_from(limit).map_err(|_| CatalogError::InvalidLimit)?;
        let rows = sqlx::query(
            "SELECT id,source_identity,target_language,title,visibility \
             FROM books WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted \
             ORDER BY id ASC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CatalogError::Unavailable)?;
        rows.into_iter().map(decode_public).collect()
    }
    async fn get_public(&self, id: u64) -> Result<Option<Book>, CatalogError> {
        let id = match i64::try_from(id) {
            Ok(n) if n > 0 => n,
            _ => return Ok(None),
        };
        sqlx::query(
            "SELECT id,source_identity,target_language,title,visibility \
             FROM books WHERE id=$1 AND visibility='public' AND NOT admin_hold AND NOT is_deleted",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CatalogError::Unavailable)?
        .map(decode_public)
        .transpose()
    }
}

/// Author-side PostgreSQL operations. Public catalog reads remain separate.
#[derive(Clone)]
pub struct PgAuthoring {
    pool: PgPool,
}

impl PgAuthoring {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A successful command creates exactly one private draft. Concurrent retries
    /// with the same payload return the original ID; changed payloads conflict.
    pub async fn create_owned_book(
        &self,
        user_id: i64,
        book: &NewBook,
    ) -> Result<i64, AuthorError> {
        if user_id <= 0 {
            return Err(AuthorError::Forbidden);
        }
        let digest = owned_book_hash::payload_hash(book);
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let claimed = sqlx::query_scalar::<_, i64>(
            "INSERT INTO book_create_commands (user_id, command_key, payload_hash)
             VALUES ($1, $2, $3)
             ON CONFLICT (user_id, command_key) DO NOTHING
             RETURNING user_id",
        )
        .bind(user_id)
        .bind(&book.command_key)
        .bind(&digest[..])
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;

        if claimed.is_none() {
            let existing = sqlx::query(
                "SELECT payload_hash, book_id FROM book_create_commands
                 WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
            )
            .bind(user_id)
            .bind(&book.command_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?
            .ok_or(AuthorError::Unavailable)?;
            let previous_digest: Vec<u8> = existing
                .try_get("payload_hash")
                .map_err(|_| AuthorError::Unavailable)?;
            if previous_digest.as_slice() != digest.as_slice() {
                return Err(AuthorError::Conflict);
            }
            let previous_id: Option<i64> = existing
                .try_get("book_id")
                .map_err(|_| AuthorError::Unavailable)?;
            return previous_id.ok_or(AuthorError::Unavailable);
        }

        let book_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO books (source_identity, target_language, title, visibility, owner_user_id, author_name, genres, tags, fandoms, description, access_level)
             VALUES ($1, $2, $3, 'draft', $4, $5, $6, $7, $8, $9, 'basic') RETURNING id",
        )
        .bind(&book.source_identity)
        .bind(&book.target_language)
        .bind(&book.title)
        .bind(user_id)
        .bind(&book.author_name)
        .bind(&book.genres)
        .bind(&book.tags)
        .bind(&book.fandoms)
        .bind(&book.description)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| match error {
            sqlx::Error::Database(ref db) if db.code().as_deref() == Some("23505") => {
                AuthorError::Conflict
            }
            _ => AuthorError::Unavailable,
        })?;
        sqlx::query(
            "UPDATE book_create_commands SET book_id=$3 WHERE user_id=$1 AND command_key=$2",
        )
        .bind(user_id)
        .bind(&book.command_key)
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(book_id)
    }

    /// Read only this author's books, with a bounded amount of HTML data.
    pub async fn list_owned_books(&self, user_id: i64) -> Result<Vec<OwnedBook>, AuthorError> {
        if user_id <= 0 {
            return Err(AuthorError::Forbidden);
        }
        let rows = sqlx::query(
            "SELECT id, title FROM books
             WHERE owner_user_id=$1 AND NOT is_deleted AND NOT admin_hold
             ORDER BY id DESC LIMIT 100",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(OwnedBook {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect()
    }

    /// Owner-scoped publication status. Guests must never infer private drafts.
    pub async fn owned_book_is_public(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<bool, AuthorError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        let status: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT is_deleted AND NOT admin_hold",
        )
        .bind(book_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        match status.as_deref() {
            Some("public") => Ok(true),
            Some("draft") => Ok(false),
            Some(_) => Err(AuthorError::Unavailable),
            None => Err(AuthorError::Missing),
        }
    }

    /// Show only chapters of the authenticated owner's book, bounded to 200 rows.
    pub async fn list_owned_chapters(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<Vec<crate::services::authoring::OwnedChapterSummary>, AuthorError> {
        use crate::services::authoring::OwnedChapterSummary;
        if user_id <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        let rows = sqlx::query(
            "SELECT c.id,c.chapter_no,c.title,c.access_level,c.visibility FROM chapters c
             JOIN books b ON b.id=c.book_id
             WHERE c.book_id=$1 AND b.owner_user_id=$2
               AND NOT c.is_deleted AND NOT b.is_deleted
               AND NOT c.admin_hold AND NOT b.admin_hold
             ORDER BY c.chapter_no,c.id LIMIT 200",
        )
        .bind(book_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                let access: String = row
                    .try_get("access_level")
                    .map_err(|_| AuthorError::Unavailable)?;
                let access = match access.as_str() {
                    "free" => ChapterAccess::Free,
                    "paid" => ChapterAccess::Paid,
                    _ => return Err(AuthorError::Unavailable),
                };
                let visibility: String = row
                    .try_get("visibility")
                    .map_err(|_| AuthorError::Unavailable)?;
                let published = match visibility.as_str() {
                    "public" => true,
                    "draft" => false,
                    _ => return Err(AuthorError::Unavailable),
                };
                Ok(OwnedChapterSummary {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    number: row
                        .try_get("chapter_no")
                        .map_err(|_| AuthorError::Unavailable)?,
                    title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                    access,
                    published,
                })
            })
            .collect()
    }

    /// Never reveal an unpublished book, including its existence, to a non-owner.
    pub async fn owned_book(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<Option<OwnedBook>, AuthorError> {
        if user_id <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT id, title FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT is_deleted AND NOT admin_hold",
        )
        .bind(book_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok(OwnedBook {
                id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }
}

/// First real writer vertical: create immutable, free chapter version and
/// publication pointer in one PostgreSQL transaction. Never expose paid text.
impl PgAuthoring {
    pub async fn create_free_chapter(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<SavedChapter, AuthorError> {
        if actor <= 0
            || request.book_id <= 0
            || request.chapter_id.is_some()
            || request.expected_revision != 0
            || request.number <= 0
            || request.access != ChapterAccess::Free
            || request.price_amount != 0
            || !valid_command_key(&request.command_key)
            || request.title.chars().count() == 0
            || request.title.chars().count() > 200
            || request.body.chars().count() == 0
            || request.body.chars().count() > 50_000
        {
            return Err(AuthorError::Invalid);
        }

        let mut hasher = Sha256::new();
        for value in [
            request.book_id.to_string(),
            request.number.to_string(),
            request.title.clone(),
            request.body.clone(),
            request.access.as_str().to_owned(),
            request.price_amount.to_string(),
        ] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        let digest: [u8; 32] = hasher.finalize().into();
        let content_sha256: [u8; 32] = Sha256::digest(request.body.as_bytes()).into();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;

        // Row lock serializes publication against deletion, owner changes and
        // concurrent chapter creation. Unowned and missing books are identical.
        let book_visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(request.book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let book_visibility = book_visibility.ok_or(AuthorError::Missing)?;
        // Adding a chapter to a public book requires a separate chapter-release
        // workflow. Saving text must not implicitly publish it.
        if book_visibility != "draft" {
            return Err(AuthorError::Conflict);
        }

        let claimed: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_save_commands (user_id, command_key, payload_hash)
             VALUES ($1,$2,$3) ON CONFLICT (user_id,command_key) DO NOTHING
             RETURNING user_id",
        )
        .bind(actor)
        .bind(&request.command_key)
        .bind(digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;

        if claimed.is_none() {
            let row = sqlx::query(
                "SELECT payload_hash, chapter_id, version_id, saved_revision
                 FROM chapter_save_commands WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
            )
            .bind(actor)
            .bind(&request.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            let prior: Vec<u8> = row
                .try_get("payload_hash")
                .map_err(|_| AuthorError::Unavailable)?;
            if prior.as_slice() != digest.as_slice() {
                return Err(AuthorError::Conflict);
            }
            return Ok(SavedChapter {
                chapter_id: row
                    .try_get::<Option<i64>, _>("chapter_id")
                    .map_err(|_| AuthorError::Unavailable)?
                    .ok_or(AuthorError::Unavailable)?,
                version_id: row
                    .try_get::<Option<i64>, _>("version_id")
                    .map_err(|_| AuthorError::Unavailable)?
                    .ok_or(AuthorError::Unavailable)?,
                revision: row
                    .try_get::<Option<i64>, _>("saved_revision")
                    .map_err(|_| AuthorError::Unavailable)?
                    .ok_or(AuthorError::Unavailable)?,
            });
        }

        let chapter_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapters (book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
             VALUES ($1,$2,$3,'public','free',0,1) RETURNING id",
        )
        .bind(request.book_id)
        .bind(request.number)
        .bind(&request.title)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| match error {
            sqlx::Error::Database(ref db) if db.code().as_deref() == Some("23505") =>
                AuthorError::Conflict,
            _ => AuthorError::Unavailable,
        })?;
        let version_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions (chapter_id,version_no,body_markdown,content_sha256)
             VALUES ($1,1,$2,$3) RETURNING id",
        )
        .bind(chapter_id)
        .bind(&request.body)
        .bind(content_sha256.as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES ($1,$2)")
            .bind(chapter_id)
            .bind(version_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;

        // No release event on save. release_book emits it atomically when
        // the owner deliberately makes the previously hidden book public.
        sqlx::query(
            "UPDATE chapter_save_commands
             SET chapter_id=$3,version_id=$4,saved_revision=1
             WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(&request.command_key)
        .bind(chapter_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(SavedChapter {
            chapter_id,
            version_id,
            revision: 1,
        })
    }

    /// Release is a deliberate owner action, not a side effect of editing.
    /// It refuses books without at least one visible free published chapter.
    pub async fn release_book(&self, actor: i64, book_id: i64) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let book_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id=$1 AND owner_user_id=$2
             AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let book_id = book_id.ok_or(AuthorError::Missing)?;
        let can_release: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM chapters c
                JOIN chapter_publications p ON p.chapter_id=c.id
                WHERE c.book_id=$1 AND c.visibility='public' AND c.access_level='free'
                  AND NOT c.admin_hold AND NOT c.is_deleted
                  AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
             )",
        )
        .bind(book_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if !can_release {
            return Err(AuthorError::Conflict);
        }
        sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
            .bind(book_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        // Release every first-visible chapter and enqueue its event atomically.
        // The CTE emits no rows on a repeated release_book command.
        sqlx::query(
            "WITH first_releases AS (
               INSERT INTO chapter_first_release_events(chapter_id,version_id)
               SELECT c.id,p.version_id FROM chapters c
               JOIN chapter_publications p ON p.chapter_id=c.id
               WHERE c.book_id=$1 AND c.visibility='public' AND c.access_level='free'
                 AND NOT c.admin_hold AND NOT c.is_deleted
                 AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               ON CONFLICT (chapter_id) DO NOTHING
               RETURNING chapter_id,version_id
             )
             INSERT INTO outbox_events
               (event_type,schema_version,aggregate_kind,aggregate_id,
                dedupe_key,payload,required_consumers)
             SELECT 'ChapterPublished',1,'chapter',r.chapter_id,
                    'chapter-published:' || r.chapter_id::text || ':' || r.version_id::text,
                    jsonb_build_object(
                        'chapter_id',r.chapter_id,'book_id',$1::bigint,
                        'version_id',r.version_id,'published_at',now(),
                        'first_release',true),
                    ARRAY['search-index-v1','subscription-inbox-v1']::text[]
             FROM first_releases AS r
             WHERE true
             ON CONFLICT (dedupe_key) DO NOTHING",
        )
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)
    }
}
