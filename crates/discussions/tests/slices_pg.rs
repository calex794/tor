//! E-01: PostgreSQL schema invariants only. E-02 must check target read rights.
use sqlx::PgPool;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn e_01_pg_comment_version_and_anchor_boundaries() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    assert!(
        url.ends_with("/tor_ci"),
        "never test against a live database"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) VALUES
        ('e01-comment@example.invalid','E01','$argon2id$fixture') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let bid: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility) VALUES
        ('ci:e01:comment','ru','E01 Book','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let cid: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
         VALUES($1,1,'E01 Chapter','public','free') RETURNING id",
    )
    .bind(bid).fetch_one(&pool)
    .await
    .unwrap();
    let vid: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'E01 content') RETURNING id",
    )
    .bind(cid).fetch_one(&pool)
    .await
    .unwrap();
    let key = "A".repeat(43);
    let comment: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_comments(author_user_id,chapter_id,version_id,body,command_key,command_hash)
         VALUES($1,$2,$3,'Комментарий',$4,decode(repeat('a',64),'hex')) RETURNING id",
    )
    .bind(uid)
    .bind(cid)
    .bind(vid)
    .bind(&key).fetch_one(&pool)
    .await
    .unwrap();
    assert!(sqlx::query(
        "INSERT INTO discussion_comments(author_user_id,chapter_id,version_id,body,command_key,command_hash)
         VALUES($1,$2,$3,'Дубль',$4,decode(repeat('a',64),'hex'))",
    )
    .bind(uid)
    .bind(cid)
    .bind(vid)
    .bind(&key).execute(&pool)
            .await
            .is_err()
    );
    sqlx::query(
        "INSERT INTO discussion_anchors(comment_id,chapter_id,version_id,block_key,start_utf16,end_utf16)
         VALUES($1,$2,$3,'p:1',0,5)",
    )
    .bind(comment)
    .bind(cid)
    .bind(vid).execute(&pool)
        .await
        .unwrap();
    assert!(sqlx::query(
        "UPDATE discussion_comments SET version_id=version_id+1 WHERE id=$1",
    )
    .bind(comment).execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "UPDATE discussion_anchors SET start_utf16=5 WHERE comment_id=$1",
    )
    .bind(comment).execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "INSERT INTO discussion_comments(author_user_id,book_id,chapter_id,version_id,body,command_key,command_hash)
         VALUES($1,$2,$3,$4,'Bad',$5,decode(repeat('b',64),'hex'))",
    )
    .bind(uid)
    .bind(bid)
    .bind(cid)
    .bind(vid)
    .bind("B".repeat(43)).execute(&pool)
            .await
            .is_err()
    );

    sqlx::query("DELETE FROM users WHERE id=$1")
    .bind(uid).execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
    .bind(bid).execute(&pool)
        .await
        .unwrap();
}
