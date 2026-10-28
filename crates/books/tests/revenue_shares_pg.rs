//! B-10: real PostgreSQL economic authority on disposable tor_ci only.
use sqlx::PgPool;
use tor_books::domain::shares::{RevenueShareError, ShareAssignment};
use tor_books::repositories::revenue_shares::PgRevenueShares;

async fn fixture(tag: &str) -> (PgPool, i64, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG URL");
    assert!(
        url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"),
        "never write a production financial database"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let mut ids = Vec::new();
    for role in ["owner", "translator"] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'B10 schedule fixture','$argon2id$ci') RETURNING id",
        )
        .bind(format!("b10-{tag}-{role}@example.test"))
        .fetch_one(&pool)
        .await
        .unwrap();
        ids.push(id);
    }
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','B10 revenue book','public',$2) RETURNING id",
    )
    .bind(format!("ci:b10:{tag}"))
    .bind(ids[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',TRUE)",
    )
    .bind(book)
    .bind(ids[1])
    .execute(&pool)
    .await
    .unwrap();
    (pool, ids[0], ids[1], book)
}

fn shares(owner: i64, contributor: i64, owner_share: i16) -> Vec<ShareAssignment> {
    vec![
        ShareAssignment {
            user_id: owner,
            share_percent: owner_share,
        },
        ShareAssignment {
            user_id: contributor,
            share_percent: 100 - owner_share,
        },
    ]
}

async fn revision(pool: &PgPool, book: i64) -> i64 {
    sqlx::query_scalar("SELECT current_revision FROM revenue_share_states WHERE book_id=$1")
        .bind(book)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn b_10_pg_atomic_revisions_preserve_100_percent_and_old_schedule() {
    let (pool, owner, editor, book) = fixture("atomic").await;
    assert_eq!(revision(&pool, book).await, 1);
    let before: (i64, i16) = sqlx::query_as(
        "SELECT user_id,share_percent FROM revenue_share_assignments
         WHERE book_id=$1 AND revision=1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, (owner, 100));
    let repo = PgRevenueShares::new(pool.clone());
    assert_eq!(
        repo.replace_schedule(owner, book, 1, &shares(owner, editor, 80), "first split")
            .await,
        Ok(2)
    );
    assert_eq!(revision(&pool, book).await, 2);
    let total: i64 = sqlx::query_scalar(
        "SELECT SUM(share_percent) FROM revenue_share_assignments
         WHERE book_id=$1 AND revision=2",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(total, 100);
    let old_owner: i16 = sqlx::query_scalar(
        "SELECT share_percent FROM revenue_share_assignments
         WHERE book_id=$1 AND revision=1 AND user_id=$2",
    )
    .bind(book)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(old_owner, 100, "old revision is immutable");

    assert_eq!(
        repo.replace_schedule(owner, book, 1, &shares(owner, editor, 70), "stale")
            .await,
        Err(RevenueShareError::Conflict)
    );
    assert_eq!(
        repo.replace_schedule(
            owner,
            book,
            2,
            &[ShareAssignment { user_id: owner, share_percent: 80 }],
            "invalid eighty"
        )
        .await,
        Err(RevenueShareError::Invalid)
    );
    assert_eq!(revision(&pool, book).await, 2);
    assert!(
        sqlx::query(
            "UPDATE revenue_share_assignments SET share_percent=50
             WHERE book_id=$1 AND revision=1"
        )
        .bind(book)
        .execute(&pool)
        .await
        .is_err(),
        "history cannot be edited"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn b_10_pg_two_concurrent_replacements_have_one_winner() {
    let (pool, owner, editor, book) = fixture("race").await;
    let repo = PgRevenueShares::new(pool.clone());
    let (a, b) = tokio::join!(
        repo.replace_schedule(owner, book, 1, &shares(owner, editor, 60), "racer A"),
        repo.replace_schedule(owner, book, 1, &shares(owner, editor, 70), "racer B")
    );
    assert_eq!([a, b].iter().filter(|v| v.is_ok()).count(), 1);
    assert_eq!(
        [a, b].iter().filter(|v| **v == Err(RevenueShareError::Conflict)).count(),
        1
    );
    assert_eq!(revision(&pool, book).await, 2);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM revenue_share_versions WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 2);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn b_10_pg_active_share_holder_cannot_be_deactivated() {
    let (pool, owner, editor, book) = fixture("staffing").await;
    let repo = PgRevenueShares::new(pool.clone());
    assert_eq!(
        repo.replace_schedule(owner, book, 1, &shares(owner, editor, 50), "economics")
            .await,
        Ok(2)
    );
    assert!(
        sqlx::query(
            "UPDATE book_members SET is_active=FALSE
             WHERE book_id=$1 AND user_id=$2",
        )
        .bind(book)
        .bind(editor)
        .execute(&pool)
        .await
        .is_err()
    );
    assert_eq!(
        repo.replace_schedule(
            owner,
            book,
            2,
            &[ShareAssignment { user_id: owner, share_percent: 100 }],
            "remove editor share"
        )
        .await,
        Ok(3)
    );
    sqlx::query(
        "UPDATE book_members SET is_active=FALSE
         WHERE book_id=$1 AND user_id=$2",
    )
    .bind(book)
    .bind(editor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.replace_schedule(owner, book, 3, &shares(owner, editor, 80), "inactive")
            .await,
        Err(RevenueShareError::InactiveRecipient)
    );
    assert_eq!(revision(&pool, book).await, 3);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn b_10_pg_legacy_book_has_no_inferred_economics() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let legacy: Option<i64> = sqlx::query_scalar(
        "SELECT s.current_revision FROM books b
         LEFT JOIN revenue_share_states s ON s.book_id=b.id
         WHERE b.id=1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(legacy, None, "never fabricate past split from mutable ACL");
    pool.close().await;
}
