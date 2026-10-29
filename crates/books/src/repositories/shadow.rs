//! Shadow catalog adapter: queries only the manually approved SQL view.
//! This adapter never selects books/chapters/payment tables directly.
use crate::{
    domain::{Book, BookKey, Visibility},
    services::{CatalogError, PublicBookRepository, MAX_PAGE_SIZE},
};
use async_trait::async_trait;
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    PgPool, Row,
};
use std::time::Duration;

#[derive(Clone)]
pub struct PgShadowCatalog {
    pool: PgPool,
}

impl PgShadowCatalog {
    /// Dependency-injection constructor; production must connect as the
    /// separate shadow_reader role via connect(), never the main tor_app user.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await?;
        let role: String = sqlx::query_scalar("SELECT current_user")
            .fetch_one(&pool)
            .await?;
        if role != "shadow_reader" {
            pool.close().await;
            return Err(sqlx::Error::Protocol(
                "Shadow runtime requires the separate shadow_reader role".into(),
            ));
        }
        let adapter = Self { pool };
        adapter.probe().await?;
        Ok(adapter)
    }

    pub async fn probe(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT id FROM shadow_public_books LIMIT 0")
            .execute(&self.pool)
            .await?;
        sqlx::query("SELECT id FROM shadow_public_chapters LIMIT 0")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn decode(row: PgRow) -> Result<Book, CatalogError> {
    let id: i64 = row.try_get("id").map_err(|_| CatalogError::Unavailable)?;
    let source: String = row
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
    let key = BookKey::new(source, language).map_err(|_| CatalogError::Unavailable)?;
    Book::new(id, key, title, Visibility::Public).map_err(|_| CatalogError::Unavailable)
}

#[async_trait]
impl PublicBookRepository for PgShadowCatalog {
    async fn list_public(&self, limit: usize) -> Result<Vec<Book>, CatalogError> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(CatalogError::InvalidLimit);
        }
        let limit = i64::try_from(limit).map_err(|_| CatalogError::InvalidLimit)?;
        let rows = sqlx::query(
            "SELECT id, source_identity, target_language, title, visibility \
             FROM shadow_public_books ORDER BY id ASC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CatalogError::Unavailable)?;
        rows.into_iter().map(decode).collect()
    }

    async fn get_public(&self, id: u64) -> Result<Option<Book>, CatalogError> {
        let id = match i64::try_from(id) {
            Ok(id) if id > 0 => id,
            _ => return Ok(None),
        };
        sqlx::query(
            "SELECT id, source_identity, target_language, title, visibility \
             FROM shadow_public_books WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CatalogError::Unavailable)?
        .map(decode)
        .transpose()
    }
}
