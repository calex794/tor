//! S009 collection ratings and comments: public-only, bounded, actor-checked.
//! No method grants reading rights or changes the collection revenue model.
use sqlx::{PgPool, Row};

use super::book_collections::CollectionError;

#[derive(Clone)]
pub struct PgCollectionFeedback {
    pool: PgPool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionComment {
    pub id: i64,
    pub author_user_id: i64,
    pub author_name: String,
    pub body: String,
}

#[derive(Debug)]
pub struct CollectionFeedback {
    pub rating_count: i64,
    pub average_rating: Option<f64>,
    pub viewer_rating: Option<i16>,
    pub comments: Vec<CollectionComment>,
}

impl PgCollectionFeedback {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Callers must not see comments of a private or deleted collection.
    pub async fn read_public(
        &self,
        collection_id: i64,
        viewer: Option<i64>,
    ) -> Result<CollectionFeedback, CollectionError> {
        if collection_id <= 0 {
            return Err(CollectionError::NotFound);
        }
        let visible: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM social_book_collections
             WHERE id=$1 AND visibility='public' AND deleted_at IS NULL",
        )
        .bind(collection_id)
        .fetch_optional(&self.pool)
        .await?;
        if visible.is_none() {
            return Err(CollectionError::NotFound);
        }
        let summary = sqlx::query(
            "SELECT COUNT(r.user_id)::BIGINT AS rating_count,
                    AVG(r.score::FLOAT8) AS average_rating
             FROM social_book_collection_ratings r
             JOIN social_book_collections c ON c.id=r.collection_id
             WHERE c.id=$1 AND c.visibility='public' AND c.deleted_at IS NULL",
        )
        .bind(collection_id)
        .fetch_one(&self.pool)
        .await?;
        let my_rating: Option<i16> = sqlx::query_scalar(
            "SELECT r.score FROM social_book_collection_ratings r
             JOIN social_book_collections c ON c.id=r.collection_id
             WHERE c.id=$1 AND r.user_id=$2
               AND c.visibility='public' AND c.deleted_at IS NULL",
        )
        .bind(collection_id)
        .bind(viewer.unwrap_or(0))
        .fetch_optional(&self.pool)
        .await?;
        let rows = sqlx::query(
            "SELECT cm.id,cm.author_user_id,u.display_name AS author_name,cm.body
             FROM social_book_collection_comments cm
             JOIN social_book_collections c ON c.id=cm.collection_id
             JOIN users u ON u.id=cm.author_user_id
             WHERE cm.collection_id=$1 AND cm.deleted_at IS NULL
               AND c.visibility='public' AND c.deleted_at IS NULL
             ORDER BY cm.id DESC LIMIT 50",
        )
        .bind(collection_id)
        .fetch_all(&self.pool)
        .await?;
        let comments = rows
            .into_iter()
            .map(|row| {
                Ok(CollectionComment {
                    id: row.try_get("id")?,
                    author_user_id: row.try_get("author_user_id")?,
                    author_name: row.try_get("author_name")?,
                    body: row.try_get("body")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok(CollectionFeedback {
            rating_count: summary.try_get("rating_count")?,
            average_rating: summary.try_get("average_rating")?,
            viewer_rating: my_rating,
            comments,
        })
    }

    /// A disabled user or non-public collection cannot acquire a rating row.
    pub async fn save_rating(
        &self,
        actor: i64,
        collection_id: i64,
        score: i16,
    ) -> Result<(), CollectionError> {
        if actor <= 0 || collection_id <= 0 || !(1..=5).contains(&score) {
            return Err(CollectionError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        Self::check_actor(&mut tx, actor).await?;
        Self::lock_public(&mut tx, collection_id).await?;
        sqlx::query(
            "INSERT INTO social_book_collection_ratings(collection_id,user_id,score)
             VALUES($1,$2,$3)
             ON CONFLICT(collection_id,user_id)
             DO UPDATE SET score=EXCLUDED.score,updated_at=clock_timestamp()",
        )
        .bind(collection_id)
        .bind(actor)
        .bind(score)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// One unique form key authorizes at most one comment with this exact body.
    pub async fn add_comment(
        &self,
        actor: i64,
        collection_id: i64,
        body: &str,
        request_key: &str,
    ) -> Result<i64, CollectionError> {
        let trimmed = body.trim();
        if actor <= 0
            || collection_id <= 0
            || trimmed.is_empty()
            || trimmed.chars().count() > 2000
            || trimmed.contains('\0')
            || request_key.len() != 43
            || !request_key
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_')
        {
            return Err(CollectionError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        Self::check_actor(&mut tx, actor).await?;
        Self::lock_public(&mut tx, collection_id).await?;
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO social_book_collection_comments
             (collection_id,author_user_id,body,request_key) VALUES($1,$2,$3,$4)
             ON CONFLICT(collection_id,author_user_id,request_key) DO NOTHING
             RETURNING id",
        )
        .bind(collection_id)
        .bind(actor)
        .bind(trimmed)
        .bind(request_key)
        .fetch_optional(&mut *tx)
        .await?;
        let id = match inserted {
            Some(id) => id,
            None => {
                let existing = sqlx::query(
                    "SELECT id,body FROM social_book_collection_comments
                     WHERE collection_id=$1 AND author_user_id=$2
                       AND request_key=$3 AND deleted_at IS NULL",
                )
                .bind(collection_id)
                .bind(actor)
                .bind(request_key)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(CollectionError::Conflict)?;
                if existing.try_get::<String, _>("body")? != trimmed {
                    return Err(CollectionError::Conflict);
                }
                existing.try_get("id")?
            }
        };
        tx.commit().await?;
        Ok(id)
    }

    /// An author can remove their comment; a collection owner can moderate it.
    /// Both operations are soft deletes and cannot reveal a private collection.
    pub async fn delete_comment(
        &self,
        actor: i64,
        collection_id: i64,
        comment_id: i64,
    ) -> Result<(), CollectionError> {
        if actor <= 0 || collection_id <= 0 || comment_id <= 0 {
            return Err(CollectionError::Invalid);
        }
        let removed = sqlx::query(
            "UPDATE social_book_collection_comments cm
             SET deleted_at=clock_timestamp()
             WHERE cm.id=$1 AND cm.collection_id=$2 AND cm.deleted_at IS NULL
               AND EXISTS(SELECT 1 FROM users u
                          WHERE u.id=$3 AND u.account_state='active')
               AND EXISTS(
                   SELECT 1 FROM social_book_collections c
                   WHERE c.id=cm.collection_id AND c.deleted_at IS NULL
                     AND (cm.author_user_id=$3 OR c.owner_user_id=$3)
               )",
        )
        .bind(comment_id)
        .bind(collection_id)
        .bind(actor)
        .execute(&self.pool)
        .await?;
        if removed.rows_affected() != 1 {
            return Err(CollectionError::NotFound);
        }
        Ok(())
    }

    async fn check_actor(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        actor: i64,
    ) -> Result<(), CollectionError> {
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut **tx)
        .await?;
        if active.is_none() {
            return Err(CollectionError::NotFound);
        }
        Ok(())
    }

    async fn lock_public(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        collection_id: i64,
    ) -> Result<(), CollectionError> {
        let visible: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM social_book_collections
             WHERE id=$1 AND visibility='public' AND deleted_at IS NULL
             FOR SHARE",
        )
        .bind(collection_id)
        .fetch_optional(&mut **tx)
        .await?;
        if visible.is_none() {
            return Err(CollectionError::NotFound);
        }
        Ok(())
    }
}
