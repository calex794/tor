pub mod catalog;
pub mod showcases;

use crate::domain::{validate_browse, validate_facets, CatalogSort, SearchError, SearchPage};
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgSearch {
    pool: PgPool,
}
impl PgSearch {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn search(
        &self,
        term: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SearchPage, SearchError> {
        self.search_filtered(term, None, after, limit).await
    }

    pub async fn search_filtered(
        &self,
        term: &str,
        language: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SearchPage, SearchError> {
        self.search_faceted(term, language, None, after, limit)
            .await
    }

    /// F-02: an exact count is requested separately from the fast page query.
    /// It has the same validation and visibility predicates as the catalog.
    pub async fn count_faceted(
        &self,
        term: &str,
        language: Option<&str>,
        genre: Option<&str>,
    ) -> Result<u64, SearchError> {
        let (_, language, genre) = validate_browse(term, language, genre, None, 1)?;
        catalog::count_filtered(
            &self.pool,
            term,
            language.as_deref(),
            genre.as_deref(),
        )
        .await
    }

    /// Legacy search stays strict: without a filter, callers must explicitly
    /// request a catalog order (the older API returned 400 for empty search).
    pub async fn search_faceted(
        &self,
        term: &str,
        language: Option<&str>,
        genre: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SearchPage, SearchError> {
        self.search_faceted_sorted(term, language, genre, after, limit, None)
            .await
    }

    /// F-02: no OFFSET, stable identity keyset, allow full-catalog paging only
    /// when the caller explicitly supplies a supported sort.
    pub async fn search_faceted_sorted(
        &self,
        term: &str,
        language: Option<&str>,
        genre: Option<&str>,
        after: Option<&str>,
        limit: usize,
        sort: Option<&str>,
    ) -> Result<SearchPage, SearchError> {
        let direction = CatalogSort::parse(sort)?;
        let (after, language, genre) = if sort.is_some() {
            validate_browse(term, language, genre, after, limit)?
        } else {
            validate_facets(term, language, genre, after, limit)?
        };
        catalog::read_page(
            &self.pool,
            term,
            language.as_deref(),
            genre.as_deref(),
            after,
            limit,
            direction,
        )
        .await
    }
}
