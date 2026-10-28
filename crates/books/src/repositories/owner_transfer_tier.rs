//! B / B-09d: canonical Book transfer tier from a locked PostgreSQL row.
//!
//! Imported books with unknown legacy access level remain NULL and cannot
//! pass the B-09 policy. No client-provided tier may be used as authority.
use crate::services::authoring::AuthorError;
use crate::services::owner_transfer_policy::AccessTier;
use sqlx::{Postgres, Transaction};

#[allow(dead_code)]
pub(crate) async fn verified_book_tier(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
) -> Result<Option<AccessTier>, AuthorError> {
    if book_id <= 0 {
        return Ok(None);
    }
    let stored: Option<Option<String>> = sqlx::query_scalar(
        "SELECT access_level FROM books
         WHERE id=$1 AND NOT admin_hold AND NOT is_deleted
         FOR UPDATE",
    )
    .bind(book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(stored
        .flatten()
        .and_then(|tier| AccessTier::from_persisted(&tier)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_book_tier_is_locked_and_unknown_legacy_is_denied() {
        let url = std::env::var("TEST_DATABASE_URL").expect("CI DB");
        assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
        let pool = PgPool::connect(&url).await.unwrap();
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('b09d-book-owner@tor-ci.example','B09D owner','$argon2id$test')
             ON CONFLICT(email_normalized) DO UPDATE
             SET display_name=EXCLUDED.display_name RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let book_id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES($1,'ru','B09D legacy unknown tier','draft',$2) RETURNING id",
        )
        .bind(format!("ci:b09d:legacy:{user}"))
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(verified_book_tier(&mut tx, book_id).await.unwrap(), None);
        tx.commit().await.unwrap();

        for (tier, expected) in [
            ("basic", AccessTier::Basic),
            ("adult", AccessTier::Adult),
            ("pro_adult", AccessTier::ProAdult),
        ] {
            sqlx::query("UPDATE books SET access_level=$2 WHERE id=$1")
                .bind(book_id)
                .bind(tier)
                .execute(&pool)
                .await
                .unwrap();
            let mut tx = pool.begin().await.unwrap();
            assert_eq!(
                verified_book_tier(&mut tx, book_id).await.unwrap(),
                Some(expected)
            );
            tx.commit().await.unwrap();
        }
        assert!(
            sqlx::query("UPDATE books SET access_level='invalid' WHERE id=$1")
                .bind(book_id)
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
            .bind(book_id)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(verified_book_tier(&mut tx, book_id).await.unwrap(), None);
        tx.commit().await.unwrap();
        pool.close().await;
    }
}
