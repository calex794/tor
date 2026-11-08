//! E-07 / S009: collections and their membership never grant paid or private Book access.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgBookCollections {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionError {
    Invalid,
    NotFound,
    Conflict,
    RateLimited,
    Unavailable,
}
impl From<sqlx::Error> for CollectionError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookCollection {
    pub id: i64,
    pub owner_user_id: i64,
    pub title: String,
    pub description: String,
    pub visibility: String,
    pub revision: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionBook {
    pub id: i64,
    pub title: String,
}
#[derive(Debug)]
pub struct CollectionDetail {
    pub collection: BookCollection,
    pub books: Vec<CollectionBook>,
    pub viewer_is_owner: bool,
}
fn clean_title(title: &str) -> Result<&str, CollectionError> {
    let trimmed = title.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 160 || trimmed.contains('\0') {
        return Err(CollectionError::Invalid);
    }
    Ok(trimmed)
}
fn valid_visibility(visibility: &str) -> bool {
    matches!(visibility, "private" | "public")
}
fn row_collection(row: sqlx::postgres::PgRow) -> Result<BookCollection, CollectionError> {
    Ok(BookCollection {
        id: row.try_get("id")?,
        owner_user_id: row.try_get("owner_user_id")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        visibility: row.try_get("visibility")?,
        revision: row.try_get("revision")?,
    })
}
impl PgBookCollections {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// An actor comes from a checked session; replay of the same form key is idempotent.
    pub async fn create(
        &self,
        actor: i64,
        title: &str,
        visibility: &str,
        request_key: &str,
    ) -> Result<BookCollection, CollectionError> {
        let title = clean_title(title)?;
        if actor <= 0
            || !valid_visibility(visibility)
            || request_key.len() != 43
            || !request_key
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_')
        {
            return Err(CollectionError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            return Err(CollectionError::NotFound);
        }
        let inserted = sqlx::query(
            "INSERT INTO social_book_collections(owner_user_id,title,visibility,request_key)
             VALUES ($1,$2,$3,$4)
             ON CONFLICT(owner_user_id,request_key) WHERE request_key IS NOT NULL DO NOTHING
             RETURNING id,owner_user_id,title,description,visibility,revision",
        )
        .bind(actor)
        .bind(title)
        .bind(visibility)
        .bind(request_key)
        .fetch_optional(&mut *tx)
        .await?;
        let row = match inserted {
            Some(row) => row,
            None => {
                let found = sqlx::query(
                    "SELECT id,owner_user_id,title,description,visibility,revision
                     FROM social_book_collections
                     WHERE owner_user_id=$1 AND request_key=$2 AND deleted_at IS NULL",
                )
                .bind(actor)
                .bind(request_key)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(CollectionError::Conflict)?;
                if found.try_get::<String, _>("title")? != title
                    || found.try_get::<String, _>("visibility")? != visibility
                {
                    return Err(CollectionError::Conflict);
                }
                found
            }
        };
        let collection = row_collection(row)?;
        tx.commit().await?;
        Ok(collection)
    }

    pub async fn list(&self, viewer: Option<i64>) -> Result<Vec<BookCollection>, CollectionError> {
        let rows = sqlx::query(
            "SELECT id,owner_user_id,title,description,visibility,revision
             FROM social_book_collections
             WHERE deleted_at IS NULL AND (visibility='public' OR owner_user_id=$1)
             ORDER BY id DESC LIMIT 50",
        )
        .bind(viewer.unwrap_or(0))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_collection).collect()
    }

    /// A bounded, owner-scoped selection list. Never reveals hidden or adult Book titles.
    pub async fn find_addable_books(
        &self,
        actor: i64,
        collection_id: i64,
        query: &str,
    ) -> Result<Vec<CollectionBook>, CollectionError> {
        let search = query.trim();
        if actor <= 0 || collection_id <= 0 || !(2..=80).contains(&search.chars().count()) {
            return Err(CollectionError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT b.id,b.title FROM books AS b
             JOIN social_book_collections AS c ON c.id=$2 AND c.owner_user_id=$1
               AND c.deleted_at IS NULL
             WHERE b.visibility='public' AND b.visibility_status='active'
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
               AND b.access_level='basic' AND NOT b.is_deleted AND NOT b.admin_hold
               AND strpos(lower(b.title),lower($3))>0
               AND NOT EXISTS (
                   SELECT 1 FROM social_book_collection_items AS i
                   WHERE i.collection_id=c.id AND i.book_id=b.id
               )
             ORDER BY b.id DESC LIMIT 20",
        )
        .bind(actor)
        .bind(collection_id)
        .bind(search)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(CollectionBook {
                    id: r.try_get("id")?,
                    title: r.try_get("title")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(CollectionError::from)
    }

    /// A hidden collection is indistinguishable from a missing one to strangers.
    /// Book titles are a public Basic-only projection here; paid chapter text
    /// and all non-Basic Book metadata stay out of this read model.
    pub async fn detail(
        &self,
        viewer: Option<i64>,
        id: i64,
    ) -> Result<CollectionDetail, CollectionError> {
        if id <= 0 {
            return Err(CollectionError::NotFound);
        }
        let viewer = viewer.unwrap_or(0);
        let row = sqlx::query(
            "SELECT id,owner_user_id,title,description,visibility,revision
             FROM social_book_collections
             WHERE id=$1 AND deleted_at IS NULL
             AND (visibility='public' OR owner_user_id=$2)",
        )
        .bind(id)
        .bind(viewer)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(CollectionError::NotFound)?;
        let collection = row_collection(row)?;
        let rows = sqlx::query(
            "SELECT b.id,b.title FROM social_book_collection_items i
             JOIN books b ON b.id=i.book_id
             WHERE i.collection_id=$1 AND b.visibility='public'
               AND b.visibility_status='active'
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
               AND b.access_level='basic'
               AND NOT b.is_deleted AND NOT b.admin_hold
             ORDER BY i.added_at,i.book_id LIMIT 100",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        let books = rows
            .into_iter()
            .map(|r| {
                Ok(CollectionBook {
                    id: r.try_get("id")?,
                    title: r.try_get("title")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok(CollectionDetail {
            viewer_is_owner: collection.owner_user_id == viewer,
            collection,
            books,
        })
    }

    async fn lock_owned(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        actor: i64,
        id: i64,
        expected: i64,
    ) -> Result<(), CollectionError> {
        if actor <= 0 || id <= 0 || expected <= 0 {
            return Err(CollectionError::Invalid);
        }
        let actual: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM social_book_collections
             WHERE id=$1 AND owner_user_id=$2 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(id)
        .bind(actor)
        .fetch_optional(&mut **tx)
        .await?;
        match actual {
            None => Err(CollectionError::NotFound),
            Some(revision) if revision != expected => Err(CollectionError::Conflict),
            Some(_) => Ok(()),
        }
    }

    pub async fn update_metadata_if_current(
        &self,
        actor: i64,
        id: i64,
        expected: i64,
        title: &str,
        visibility: &str,
    ) -> Result<i64, CollectionError> {
        let title = clean_title(title)?;
        if !valid_visibility(visibility) {
            return Err(CollectionError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        Self::lock_owned(&mut tx, actor, id, expected).await?;
        let revision: i64 = sqlx::query_scalar(
            "UPDATE social_book_collections SET title=$2,visibility=$3,
             revision=revision+1,updated_at=clock_timestamp()
             WHERE id=$1 RETURNING revision",
        )
        .bind(id)
        .bind(title)
        .bind(visibility)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(revision)
    }

    pub async fn add_book(
        &self,
        actor: i64,
        id: i64,
        expected: i64,
        book_id: i64,
    ) -> Result<i64, CollectionError> {
        if book_id <= 0 {
            return Err(CollectionError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        Self::lock_owned(&mut tx, actor, id, expected).await?;
        // Only publicly eligible Basic books enter a collection. A future
        // adult projection must carry the full S020 viewer policy end-to-end.
        let visible: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b WHERE b.id=$1
               AND b.visibility='public' AND b.visibility_status='active'
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted') AND b.access_level='basic'
               AND NOT b.is_deleted AND NOT b.admin_hold FOR SHARE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await?;
        if visible.is_none() {
            return Err(CollectionError::NotFound);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM social_book_collection_items WHERE collection_id=$1",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if count >= 100 {
            return Err(CollectionError::Invalid);
        }
        let inserted = sqlx::query(
            "INSERT INTO social_book_collection_items(collection_id,book_id)
             VALUES ($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(book_id)
        .execute(&mut *tx)
        .await?;
        let revision = if inserted.rows_affected() == 1 {
            sqlx::query_scalar::<_, i64>(
                "UPDATE social_book_collections SET revision=revision+1,
                 updated_at=clock_timestamp() WHERE id=$1 RETURNING revision",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await?
        } else {
            expected
        };
        tx.commit().await?;
        Ok(revision)
    }

    pub async fn remove_book_if_current(
        &self,
        actor: i64,
        id: i64,
        expected: i64,
        book_id: i64,
    ) -> Result<i64, CollectionError> {
        if book_id <= 0 {
            return Err(CollectionError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        Self::lock_owned(&mut tx, actor, id, expected).await?;
        let deleted = sqlx::query(
            "DELETE FROM social_book_collection_items WHERE collection_id=$1 AND book_id=$2",
        )
        .bind(id)
        .bind(book_id)
        .execute(&mut *tx)
        .await?;
        let revision = if deleted.rows_affected() == 1 {
            sqlx::query_scalar::<_, i64>(
                "UPDATE social_book_collections SET revision=revision+1,
                 updated_at=clock_timestamp() WHERE id=$1 RETURNING revision",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await?
        } else {
            expected
        };
        tx.commit().await?;
        Ok(revision)
    }

    pub async fn delete_owned_collection(
        &self,
        actor: i64,
        id: i64,
        expected: i64,
    ) -> Result<(), CollectionError> {
        let mut tx = self.pool.begin().await?;
        Self::lock_owned(&mut tx, actor, id, expected).await?;
        sqlx::query(
            "UPDATE social_book_collections
             SET deleted_at=clock_timestamp(),revision=revision+1,updated_at=clock_timestamp()
             WHERE id=$1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
