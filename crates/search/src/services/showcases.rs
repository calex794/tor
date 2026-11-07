//! F-04: latest added books and genre spotlights, without a stale public cache.
//! "Latest added" is ordered by the canonical book identity, NOT publication date.
use crate::domain::{normalize_genre, SearchBook, SearchError};
use async_trait::async_trait;
use std::collections::HashSet;

/// The repository MUST reject draft, held and deleted books before LIMIT.
/// HTTP and the read service cannot be trusted to filter after paging.
#[async_trait]
pub trait ShowcaseRepository: Send + Sync {
    async fn latest_public(
        &self,
        genre: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchBook>, SearchError>;
}

/// Homepage and genre components should never request more than 24 cards.
pub const MAX_SHOWCASE_SIZE: usize = 24;

pub struct RecentShowcases<'a> {
    repository: &'a dyn ShowcaseRepository,
}

impl<'a> RecentShowcases<'a> {
    pub fn new(repository: &'a dyn ShowcaseRepository) -> Self {
        Self { repository }
    }

    /// Live PostgreSQL reads are intentional: no unsafe cache invalidation on
    /// unpublish/hold/deletion. Popularity and first-release time are separate
    /// capabilities and MUST NOT be inferred from insertion order.
    pub async fn latest_added(
        &self,
        genre: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchBook>, SearchError> {
        if !(1..=MAX_SHOWCASE_SIZE).contains(&limit) {
            return Err(SearchError::Invalid);
        }
        let genre = normalize_genre(genre)?;
        let books = self
            .repository
            .latest_public(genre.as_deref(), limit)
            .await?;
        if books.len() > limit
            || books.iter().any(|book| book.id == 0)
            || books.windows(2).any(|pair| pair[0].id <= pair[1].id)
        {
            return Err(SearchError::Unavailable);
        }
        Ok(books)
    }
}

/// Read model for real first chapter releases, never inferred from book IDs.
#[async_trait]
pub trait ReleaseShowcaseRepository: Send + Sync {
    async fn recent_free_releases(&self, limit: usize) -> Result<Vec<SearchBook>, SearchError>;
}

pub struct ReleaseShowcases<'a> {
    repository: &'a dyn ReleaseShowcaseRepository,
}

impl<'a> ReleaseShowcases<'a> {
    pub fn new(repository: &'a dyn ReleaseShowcaseRepository) -> Self {
        Self { repository }
    }

    /// Each book may appear only once, even when many chapters were published.
    pub async fn recent_free_releases(&self, limit: usize) -> Result<Vec<SearchBook>, SearchError> {
        if !(1..=MAX_SHOWCASE_SIZE).contains(&limit) {
            return Err(SearchError::Invalid);
        }
        let books = self.repository.recent_free_releases(limit).await?;
        let mut ids = HashSet::new();
        if books.len() > limit
            || books
                .iter()
                .any(|book| book.id == 0 || !ids.insert(book.id))
        {
            return Err(SearchError::Unavailable);
        }
        Ok(books)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ReversedRows;

    #[async_trait]
    impl ShowcaseRepository for ReversedRows {
        async fn latest_public(
            &self,
            _genre: Option<&str>,
            _limit: usize,
        ) -> Result<Vec<SearchBook>, SearchError> {
            Ok(vec![
                SearchBook {
                    id: 1,
                    title: "A".into(),
                    language: "ru".into(),
                    author_name: String::new(),
                    genres: Vec::new(),
                    tags: Vec::new(),
                    fandoms: Vec::new(),
                },
                SearchBook {
                    id: 2,
                    title: "B".into(),
                    language: "ru".into(),
                    author_name: String::new(),
                    genres: Vec::new(),
                    tags: Vec::new(),
                    fandoms: Vec::new(),
                },
            ])
        }
    }

    #[tokio::test]
    async fn f_04_rejects_out_of_order_repository_and_bad_filters() {
        let showcase = RecentShowcases::new(&ReversedRows);
        assert_eq!(
            showcase.latest_added(None, 2).await,
            Err(SearchError::Unavailable)
        );
        assert_eq!(
            showcase.latest_added(None, 0).await,
            Err(SearchError::Invalid)
        );
        assert_eq!(
            showcase.latest_added(None, 25).await,
            Err(SearchError::Invalid)
        );
        assert_eq!(
            showcase.latest_added(Some("genre,other"), 2).await,
            Err(SearchError::Invalid)
        );
    }

    struct DuplicateReleases;

    #[async_trait]
    impl ReleaseShowcaseRepository for DuplicateReleases {
        async fn recent_free_releases(
            &self,
            _limit: usize,
        ) -> Result<Vec<SearchBook>, SearchError> {
            let book = SearchBook {
                id: 42,
                title: "Выпуск".into(),
                language: "ru".into(),
                author_name: String::new(),
                genres: Vec::new(),
                tags: Vec::new(),
                fandoms: Vec::new(),
            };
            Ok(vec![book.clone(), book])
        }
    }

    #[tokio::test]
    async fn releases_reject_duplicates_and_invalid_limits() {
        let showcase = ReleaseShowcases::new(&DuplicateReleases);
        assert_eq!(
            showcase.recent_free_releases(0).await,
            Err(SearchError::Invalid)
        );
        assert_eq!(
            showcase.recent_free_releases(25).await,
            Err(SearchError::Invalid)
        );
        assert_eq!(
            showcase.recent_free_releases(12).await,
            Err(SearchError::Unavailable)
        );
    }
}
