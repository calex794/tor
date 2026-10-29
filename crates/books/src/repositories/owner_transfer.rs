//! B-09c / S030: atomic owner-row swap under one PostgreSQL Book lock.
//!
//! This is deliberately a *sealed*, crate-internal persistence primitive.
//! Before it may be called from a production B service, that service must
//! prove recipient effective tier, Book tier, active Layer rights (including
//! those inherited from historical Layer compositions), financial snapshots,
//! fresh session/step-up and the exact Book revision in the SAME boundary.
//! No HTTP or public export is allowed to construct the permit yet.
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};

/// No production constructor: only the test below can mint this capability.
/// Do not expose it until the account/Layer/revenue verification ports exist.
pub(crate) struct TrustedTransferPermit {
    _issuer: (),
}

/// All A/B/D/B-10 checks and their row locks must be performed by the
/// caller in this SAME transaction before it can construct a trusted permit.
/// This primitive only updates owner rows; it is never an HTTP permission.
#[allow(dead_code)]
pub(crate) async fn swap_owner_in_verified_transaction(
    tx: &mut Transaction<'_, Postgres>,
    actor_id: i64,
    book_id: i64,
    expected_owner_id: i64,
    recipient_id: i64,
    _permit: TrustedTransferPermit,
) -> Result<(), AuthorError> {
    if actor_id <= 0 || book_id <= 0 || recipient_id <= 0 || expected_owner_id <= 0 {
        return Err(AuthorError::Invalid);
    }
    if actor_id != expected_owner_id || expected_owner_id == recipient_id {
        return Err(AuthorError::Forbidden);
    }
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT owner_user_id FROM books
         WHERE id=$1 AND NOT admin_hold AND NOT is_deleted
         FOR UPDATE",
    )
    .bind(book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if current != Some(expected_owner_id) {
        return Err(AuthorError::Conflict);
    }

    let active_owner: Option<i64> = sqlx::query_scalar(
        "SELECT user_id FROM book_members
         WHERE book_id=$1 AND is_active AND role='owner' FOR UPDATE",
    )
    .bind(book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if active_owner != Some(expected_owner_id) {
        return Err(AuthorError::Conflict);
    }

    // Demote first so the partial unique OWNER index cannot observe two
    // owners, even briefly within this transaction.
    let demoted: Option<i64> = sqlx::query_scalar(
        "UPDATE book_members SET role='translator',updated_at=now()
         WHERE book_id=$1 AND user_id=$2 AND is_active AND role='owner'
         RETURNING user_id",
    )
    .bind(book_id)
    .bind(expected_owner_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if demoted != Some(expected_owner_id) {
        return Err(AuthorError::Conflict);
    }

    let promoted: Option<i64> = sqlx::query_scalar(
        "INSERT INTO book_members(book_id,user_id,role,is_active,share_percent)
         SELECT $1,id,'owner',TRUE,0 FROM users WHERE id=$2
         ON CONFLICT(book_id,user_id) DO UPDATE
         SET role='owner',is_active=TRUE,updated_at=now()
         RETURNING user_id",
    )
    .bind(book_id)
    .bind(recipient_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if promoted != Some(recipient_id) {
        return Err(AuthorError::Missing);
    }

    let cas: Option<i64> = sqlx::query_scalar(
        "UPDATE books SET owner_user_id=$3
         WHERE id=$1 AND owner_user_id=$2
           AND NOT admin_hold AND NOT is_deleted
         RETURNING id",
    )
    .bind(book_id)
    .bind(expected_owner_id)
    .bind(recipient_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if cas != Some(book_id) {
        return Err(AuthorError::Conflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{PgPool, Row};

    async fn pool() -> PgPool {
        let url = std::env::var("TEST_DATABASE_URL").expect("PG CI URL");
        assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
        PgPool::connect(&url)
            .await
            .expect("isolated test PostgreSQL")
    }

    async fn user(pool: &PgPool, id: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'B-09 CAS fixture','$argon2id$test')
             ON CONFLICT(email_normalized) DO UPDATE
             SET display_name=EXCLUDED.display_name RETURNING id",
        )
        .bind(format!("b09c-{id}@tor-ci.example"))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn fixture(pool: &PgPool, label: &str) -> (i64, i64, i64, i64) {
        let from = user(pool, &format!("{label}-from")).await;
        let to_a = user(pool, &format!("{label}-to-a")).await;
        let to_b = user(pool, &format!("{label}-to-b")).await;
        let book_id = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES($1,'ru','B09 atomic owner swap','draft',$2) RETURNING id",
        )
        .bind(format!("ci:b09c:{label}:owner:{from}"))
        .bind(from)
        .fetch_one(pool)
        .await
        .unwrap();
        (book_id, from, to_a, to_b)
    }

    fn permit() -> TrustedTransferPermit {
        TrustedTransferPermit { _issuer: () }
    }

    /// Test-only stand-in for the future B-owned service: one database
    /// transaction must encompass evidence validation, owner swap and commit.
    async fn fixture_transfer(
        pool: &PgPool,
        actor: i64,
        book_id: i64,
        expected_owner: i64,
        recipient: i64,
    ) -> Result<(), AuthorError> {
        let mut tx = pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        let result = swap_owner_in_verified_transaction(
            &mut tx,
            actor,
            book_id,
            expected_owner,
            recipient,
            permit(),
        )
        .await;
        match result {
            Ok(()) => tx.commit().await.map_err(|_| AuthorError::Unavailable),
            Err(error) => {
                tx.rollback().await.map_err(|_| AuthorError::Unavailable)?;
                Err(error)
            }
        }
    }

    async fn owner_and_count(pool: &PgPool, book_id: i64) -> (i64, i64) {
        let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
            .bind(book_id)
            .fetch_one(pool)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM book_members WHERE book_id=$1
             AND role='owner' AND is_active",
        )
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap();
        (owner, count)
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_owner_swap_preserves_membership_shares_and_one_owner() {
        let pool = pool().await;
        let (book_id, from, to, _) = fixture(&pool, "success").await;
        sqlx::query(
            "UPDATE book_members SET share_percent=21
             WHERE book_id=$1 AND user_id=$2",
        )
        .bind(book_id)
        .bind(from)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO book_members(book_id,user_id,role,share_percent)
             VALUES($1,$2,'translator',13)",
        )
        .bind(book_id)
        .bind(to)
        .execute(&pool)
        .await
        .unwrap();
        fixture_transfer(&pool, from, book_id, from, to)
            .await
            .unwrap();
        assert_eq!(owner_and_count(&pool, book_id).await, (to, 1));
        let rows = sqlx::query(
            "SELECT user_id,role,is_active,share_percent FROM book_members
             WHERE book_id=$1 ORDER BY user_id",
        )
        .bind(book_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            let id: i64 = row.get("user_id");
            let role: String = row.get("role");
            let share: i16 = row.get("share_percent");
            assert!(row.get::<bool, _>("is_active"));
            if id == from {
                assert_eq!(role, "translator");
                assert_eq!(share, 21);
            } else {
                assert_eq!(id, to);
                assert_eq!(role, "owner");
                assert_eq!(share, 13);
            }
        }
        assert_eq!(
            fixture_transfer(&pool, from, book_id, from, to).await,
            Err(AuthorError::Conflict),
            "former owner cannot replay the transfer"
        );
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_owner_swap_missing_recipient_rolls_back_demote() {
        let pool = pool().await;
        let (book_id, from, _, _) = fixture(&pool, "rollback").await;
        assert_eq!(
            fixture_transfer(&pool, from, book_id, from, i64::MAX - 5).await,
            Err(AuthorError::Missing)
        );
        assert_eq!(owner_and_count(&pool, book_id).await, (from, 1));
        let role: String =
            sqlx::query_scalar("SELECT role FROM book_members WHERE book_id=$1 AND user_id=$2")
                .bind(book_id)
                .bind(from)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(role, "owner");
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_owner_swap_racing_cas_has_one_winner() {
        let pool = pool().await;
        let (book_id, from, to_a, to_b) = fixture(&pool, "race").await;
        let (a, b) = tokio::join!(
            fixture_transfer(&pool, from, book_id, from, to_a),
            fixture_transfer(&pool, from, book_id, from, to_b)
        );
        assert_eq!([a, b].iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            [a, b]
                .iter()
                .filter(|r| **r == Err(AuthorError::Conflict))
                .count(),
            1
        );
        let (actual_owner, count) = owner_and_count(&pool, book_id).await;
        assert!(actual_owner == to_a || actual_owner == to_b);
        assert_eq!(count, 1);
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_owner_swap_denies_stale_actor_and_held_book() {
        let pool = pool().await;
        let (book_id, from, to, outsider) = fixture(&pool, "deny").await;
        assert_eq!(
            fixture_transfer(&pool, outsider, book_id, from, to).await,
            Err(AuthorError::Forbidden)
        );
        sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
            .bind(book_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            fixture_transfer(&pool, from, book_id, from, to).await,
            Err(AuthorError::Conflict)
        );
        assert_eq!(owner_and_count(&pool, book_id).await, (from, 1));
        pool.close().await;
    }
}
