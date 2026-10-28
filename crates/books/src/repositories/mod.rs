pub mod book_members;
pub mod cover_slots;
// B-09c sealed ownership-transfer transaction. Not exposed to HTTP.
pub(crate) mod owner_transfer;
// B-09d canonical Book tier, locked under caller-owned transfer transaction.
pub(crate) mod owner_transfer_tier;
pub mod paid_storage;
pub mod postgres;
pub mod revenue_shares;
pub mod shadow;
pub use postgres::PgCatalog;
pub use shadow::PgShadowCatalog;

use crate::domain::{Book, Visibility};
use crate::services::{CatalogError, PublicBookRepository};
use async_trait::async_trait;
use std::sync::Arc;

/// Temporary local adapter: production catalog must use PostgreSQL and real ACLs.
#[derive(Clone, Default)]
pub struct InMemoryCatalog {
    books: Arc<Vec<Book>>,
}

impl InMemoryCatalog {
    /// The demo uses fixed anonymous data; production state is not persisted here.
    pub fn demo() -> Self {
        use crate::domain::BookKey;
        let rows = [
            (1, "Первое знакомство", Visibility::Public),
            (2, "Книжная полка", Visibility::Public),
            (3, "Закрытый черновик", Visibility::Draft),
        ];
        let books = rows
            .into_iter()
            .map(|(id, title, visibility)| {
                Book::new(
                    id,
                    BookKey::new(format!("demo:{id}"), "ru").expect("demo key"),
                    title,
                    visibility,
                )
                .expect("demo book")
            })
            .collect();
        Self::new(books)
    }

    pub fn new(books: Vec<Book>) -> Self {
        Self {
            books: Arc::new(books),
        }
    }
}

#[async_trait]
impl PublicBookRepository for InMemoryCatalog {
    async fn list_public(&self, limit: usize) -> Result<Vec<Book>, CatalogError> {
        Ok(self
            .books
            .iter()
            .filter(|book| book.visibility() == Visibility::Public)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn get_public(&self, id: u64) -> Result<Option<Book>, CatalogError> {
        Ok(self
            .books
            .iter()
            .find(|book| book.id() == id && book.visibility() == Visibility::Public)
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::BookKey;

    #[tokio::test]
    async fn never_returns_drafts() {
        let draft = Book::new(
            1,
            BookKey::new("source:1", "ru").expect("key"),
            "Draft",
            Visibility::Draft,
        )
        .expect("book");
        let published = Book::new(
            2,
            BookKey::new("source:2", "ru").expect("key"),
            "Published",
            Visibility::Public,
        )
        .expect("book");
        let repo = InMemoryCatalog::new(vec![draft, published]);
        let found = repo.list_public(10).await.expect("query");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title(), "Published");
    }
}
