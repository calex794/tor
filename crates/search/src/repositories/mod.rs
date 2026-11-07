pub mod catalog;
pub mod catalog_window;
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
    /// Per-request authentication scope. Never shared or mutated across actors.
    viewer_digest: Option<[u8; 32]>,
}
impl PgSearch {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            viewer_digest: None,
        }
    }

    /// Only a server-derived session token digest may select the personalized
    /// catalogue. Invalid/revoked sessions still see only anonymous basic rows.
    pub fn with_session_digest(&self, digest: Option<[u8; 32]>) -> Self {
        Self {
            pool: self.pool.clone(),
            viewer_digest: digest,
        }
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
            self.viewer_digest.as_ref(),
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
        let normalized = CatalogFacets {
            term: filters.term,
            language: language.as_deref(),
            genre: genre.as_deref(),
            tag: tag.as_deref(),
            fandom: fandom.as_deref(),
        };
        if lexical {
            return catalog::read_lexical_page(
                &self.pool,
                normalized,
                after,
                limit,
                direction,
                self.viewer_digest.as_ref(),
            )
            .await;
        }
        catalog::read_page(
            &self.pool,
            normalized,
            after_id,
            limit,
            direction,
            self.viewer_digest.as_ref(),
        )
        .await
    }
}

const SEO_PUBLIC_FROM: &str = "
    FROM books b
    WHERE b.visibility='public' AND b.visibility_status='active'
      AND b.access_level='basic'
      AND (b.seo_index_owner='pandaolate'
           OR (b.seo_index_owner='shadow' AND NOT b.shadow_enabled))
      AND b.lifecycle_status<>'pending_deletion'
      AND NOT b.admin_hold AND NOT b.is_deleted
      AND EXISTS (
        SELECT 1 FROM chapters c
        JOIN chapter_publications p ON p.chapter_id=c.id
        JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
        WHERE c.book_id=b.id AND c.visibility='public'
          AND c.access_level='free' AND NOT c.admin_hold AND NOT c.is_deleted
          AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
      )";

impl PgSearch {
    /// Maximum identity of a publicly indexable book. Used only to partition
    /// the sitemap; gaps or withdrawn books never expose their metadata.
    pub async fn seo_max_public_book_id(&self) -> Result<u64, SearchError> {
        if self.viewer_digest.is_some() {
            return Err(SearchError::Invalid);
        }
        let sql = format!("SELECT MAX(b.id) {SEO_PUBLIC_FROM}");
        let maximum: Option<i64> = sqlx::query_scalar(&sql)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| SearchError::Unavailable)?;
        u64::try_from(maximum.unwrap_or(0)).map_err(|_| SearchError::Unavailable)
    }

    /// Fixed 10k-ID ranges are bounded keyset reads, not OFFSET scans. This
    /// supports large catalogues and remains stable as older books are removed.
    pub async fn seo_public_book_ids_in_bucket(
        &self,
        bucket: u32,
    ) -> Result<Vec<u64>, SearchError> {
        if self.viewer_digest.is_some() {
            return Err(SearchError::Invalid);
        }
        let start = i64::from(bucket) * 10_000 + 1;
        let end = start + 10_000;
        let sql =
            format!("SELECT b.id {SEO_PUBLIC_FROM} AND b.id >= $1 AND b.id < $2 ORDER BY b.id");
        let rows: Vec<i64> = sqlx::query_scalar(&sql)
            .bind(start)
            .bind(end)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| SearchError::Unavailable)?;
        rows.into_iter()
            .map(|id| u64::try_from(id).map_err(|_| SearchError::Unavailable))
            .collect()
    }

    /// The same anonymous publication gate used by sitemap rows. It is
    /// intentionally independent from logged-in 18+/21+ catalogue access.
    pub async fn seo_public_book_indexable(&self, book_id: u64) -> Result<bool, SearchError> {
        if self.viewer_digest.is_some() {
            return Err(SearchError::Invalid);
        }
        let id = i64::try_from(book_id).map_err(|_| SearchError::Invalid)?;
        let sql = format!("SELECT EXISTS(SELECT 1 {SEO_PUBLIC_FROM} AND b.id=$1)");
        sqlx::query_scalar(&sql)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| SearchError::Unavailable)
    }
}
