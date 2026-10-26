//! Public catalog SQLx adapter, ported from bokarust@d11b27b.
use crate::{
    domain::{Book, BookKey, Visibility},
    services::{
        authoring::{AuthorError, NewBook, OwnedBook},
        CatalogError, PublicBookRepository, MAX_PAGE_SIZE,
    },
};
use async_trait::async_trait;
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    PgPool, Row,
};
use sha2::{Digest, Sha256};
use std::time::Duration;
const REQUIRED_MIGRATION: i64 = 202610130003;

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
        if applied.is_none() {
            return Err(sqlx::Error::Protocol(
                "TOR schema must have all nine historical SQLx migrations".into(),
            ));
        }
        sqlx::query(
            "SELECT id, source_identity, target_language, title, visibility \
             FROM books WHERE visibility='public' AND NOT admin_hold \
             AND NOT is_deleted LIMIT 0",
        )
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
        let mut hasher = Sha256::new();
        for part in [&book.source_identity, &book.target_language, &book.title] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        let digest: [u8; 32] = hasher.finalize().into();
        let mut tx = self.pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
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
            let previous_digest: Vec<u8> =
                existing.try_get("payload_hash").map_err(|_| AuthorError::Unavailable)?;
            if previous_digest.as_slice() != digest {
                return Err(AuthorError::Conflict);
            }
            let previous_id: Option<i64> =
                existing.try_get("book_id").map_err(|_| AuthorError::Unavailable)?;
            return previous_id.ok_or(AuthorError::Unavailable);
        }

        let book_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO books (source_identity, target_language, title, visibility, owner_user_id)
             VALUES ($1, $2, $3, 'draft', $4) RETURNING id",
        )
        .bind(&book.source_identity)
        .bind(&book.target_language)
        .bind(&book.title)
        .bind(user_id)
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
