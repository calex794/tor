pub mod catalog;
pub mod showcases;

use crate::domain::{normalize_genre, validate_browse, CatalogSort, SearchError, SearchPage};
use sqlx::PgPool;

/// B-owned discovery fields read by F.
#[derive(Clone, Copy)]
pub struct CatalogFacets<'a> {
    pub term: &'a str,
    pub language: Option<&'a str>,
    pub genre: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub fandom: Option<&'a str>,
}

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

    /// Optional exact count; no hidden, held, or deleted books.
    pub async fn count_faceted(
        &self,
        term: &str,
        language: Option<&str>,
        genre: Option<&str>,
    ) -> Result<u64, SearchError> {
        self.count_catalog(CatalogFacets {
            term,
            language,
            genre,
            tag: None,
            fandom: None,
        })
        .await
    }

    pub async fn count_catalog(&self, filters: CatalogFacets<'_>) -> Result<u64, SearchError> {
        let (_, language, genre) =
            validate_browse(filters.term, filters.language, filters.genre, None, 1)?;
        let tag = normalize_genre(filters.tag)?;
        let fandom = normalize_genre(filters.fandom)?;
        catalog::count_filtered(
            &self.pool,
            filters.term,
            language.as_deref(),
            genre.as_deref(),
            tag.as_deref(),
            fandom.as_deref(),
        )
        .await
    }

    pub async fn taxonomy_options(&self) -> Result<catalog::TaxonomyOptions, SearchError> {
        catalog::taxonomy_options(&self.pool).await
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

    /// Old public search methods continue to work unchanged.
    pub async fn search_faceted_sorted(
        &self,
        term: &str,
        language: Option<&str>,
        genre: Option<&str>,
        after: Option<&str>,
        limit: usize,
        sort: Option<&str>,
    ) -> Result<SearchPage, SearchError> {
        self.search_catalog(
            CatalogFacets {
                term,
                language,
                genre,
                tag: None,
                fandom: None,
            },
            after,
            limit,
            sort,
        )
        .await
    }

    /// All filtering is SQL-bound before LIMIT or cursor application.
    pub async fn search_catalog(
        &self,
        filters: CatalogFacets<'_>,
        after: Option<&str>,
        limit: usize,
        sort: Option<&str>,
    ) -> Result<SearchPage, SearchError> {
        let direction = CatalogSort::parse(sort)?;
        let lexical = matches!(direction, CatalogSort::Title | CatalogSort::Author);
        let numeric_cursor = if lexical { None } else { after };
        let (after_id, language, genre) = validate_browse(
            filters.term,
            filters.language,
            filters.genre,
            numeric_cursor,
            limit,
        )?;
        let tag = normalize_genre(filters.tag)?;
        let fandom = normalize_genre(filters.fandom)?;
        if sort.is_none()
            && filters.term.trim().is_empty()
            && language.is_none()
            && genre.is_none()
            && tag.is_none()
            && fandom.is_none()
        {
            return Err(SearchError::Invalid);
        }
        if lexical {
            return catalog::read_lexical_page(
                &self.pool,
                filters.term,
                language.as_deref(),
                genre.as_deref(),
                tag.as_deref(),
                fandom.as_deref(),
                after,
                limit,
                direction,
            )
            .await;
        }
        catalog::read_page(
            &self.pool,
            filters.term,
            language.as_deref(),
            genre.as_deref(),
            tag.as_deref(),
            fandom.as_deref(),
            after_id,
            limit,
            direction,
        )
        .await
    }
}
