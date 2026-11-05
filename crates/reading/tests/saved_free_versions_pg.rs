//! C-08 / BR-060 phase 1: a saved free edition is pinned separately from
//! a Book bookmark and from purchased entitlements. No archive reader yet.
#![forbid(unsafe_code)]

use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci migrated by I"]
async fn c08_saved_free_version_is_owner_scoped_immutable_and_retained() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable DB");
    assert!(url.ends_with("/tor_ci"), "persistent DB forbidden");
    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("tor_app connection");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Free archive owner','$argon2id$test') RETURNING id",
    )
    .bind(format!("saved-free-owner-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Other reader','$argon2id$test') RETURNING id",
    )
    .bind(format!("saved-free-other-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level) \
         VALUES($1,'ru','Saved free V1','public','basic') RETURNING id",
    )
    .bind(format!("ci:saved-free:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) \
         VALUES($1,1,'Free chapter','public','free') RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
         VALUES($1,1,'Saved original V1') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
         VALUES($1,2,'Newer published V2') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id) VALUES($1,$2,$3)",
    )
    .bind(owner)
    .bind(chapter)
    .bind(v1)
    .execute(&pool)
    .await
    .unwrap();

    // A repeated save after V2 cannot rewrite the reader's earlier V1.
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    let replay = sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id) \
         VALUES($1,$2,$3) ON CONFLICT (user_id,chapter_id) DO NOTHING",
    )
    .bind(owner)
    .bind(chapter)
    .bind(v2)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(replay.rows_affected(), 0);
    let pinned: i64 = sqlx::query_scalar(
        "SELECT version_id FROM reader_saved_free_chapters \
         WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(owner)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pinned, v1);
    let forbidden = sqlx::query(
        "UPDATE reader_saved_free_chapters SET version_id=$3 \
         WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(owner)
    .bind(chapter)
    .bind(v2)
    .execute(&pool)
    .await
    .expect_err("saved editions must not be repointed");
    assert!(forbidden.to_string().contains("immutable"));
    let other_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_saved_free_chapters WHERE user_id=$1")
            .bind(other)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(other_count, 0);
    let purchases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM library_purchased_chapters WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(owner)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purchases, 0, "a saved-free pin is never a purchase");

    // Current public visibility and ownership are different from retention.
    sqlx::query(
        "UPDATE books SET lifecycle_status='pending_deletion', is_deleted=TRUE WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let retained: i64 = sqlx::query_scalar(
        "SELECT version_id FROM reader_saved_free_chapters \
         WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(owner)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retained, v1);
    assert!(
        sqlx::query("DELETE FROM chapters WHERE id=$1")
            .bind(chapter)
            .execute(&pool)
            .await
            .is_err(),
        "physical deletion may not cascade through a reader's pinned version",
    );

    // A trusted retention/erasure path can explicitly release the pin.
    sqlx::query("DELETE FROM reader_saved_free_chapters WHERE user_id=$1 AND chapter_id=$2")
        .bind(owner)
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1 OR id=$2")
        .bind(owner)
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
}
