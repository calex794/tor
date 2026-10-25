use async_trait::async_trait;
use bokarust_domain::books::{Book, Visibility};
use std::{error::Error, fmt};

pub const DEFAULT_PAGE_SIZE: usize = 20;
pub const MAX_PAGE_SIZE: usize = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogError {
    InvalidLimit,
    Unavailable,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for CatalogError {}

/// The port defines a query for public books only; no caller can widen visibility.
#[async_trait]
pub trait PublicBookRepository: Send + Sync {
    async fn list_public(&self, limit: usize) -> Result<Vec<Book>, CatalogError>;
}

pub struct ListPublicBooks<'a> {
    repository: &'a dyn PublicBookRepository,
}

impl<'a> ListPublicBooks<'a> {
    pub fn new(repository: &'a dyn PublicBookRepository) -> Self {
        Self { repository }
    }

    pub async fn execute(&self, limit: usize) -> Result<Vec<Book>, CatalogError> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(CatalogError::InvalidLimit);
        }
        let books = self.repository.list_public(limit).await?;
        // A defective adapter is not permitted to expand the public read model.
        if books
            .iter()
            .any(|book| book.visibility() != Visibility::Public)
        {
            return Err(CatalogError::Unavailable);
        }
        Ok(books.into_iter().take(limit).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bokarust_domain::books::BookKey;

    struct DefectiveRepository;

    #[async_trait]
    impl PublicBookRepository for DefectiveRepository {
        async fn list_public(&self, _limit: usize) -> Result<Vec<Book>, CatalogError> {
            let key = BookKey::new("source:1", "ru").expect("key");
            Ok(vec![
                Book::new(1, key, "Hidden", Visibility::Draft).expect("book")
            ])
        }
    }

    #[tokio::test]
    async fn rejects_unbounded_read() {
        let use_case = ListPublicBooks::new(&DefectiveRepository);
        assert_eq!(
            use_case.execute(MAX_PAGE_SIZE + 1).await,
            Err(CatalogError::InvalidLimit)
        );
    }

    #[tokio::test]
    async fn fails_closed_when_adapter_returns_private_book() {
        let use_case = ListPublicBooks::new(&DefectiveRepository);
        assert_eq!(use_case.execute(20).await, Err(CatalogError::Unavailable));
    }
}
