//! Public catalog SQLx adapter, ported from bokarust@d11b27b.
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
