//! E-12 integration: immutable version, idempotent proposals, author ACL.
#![forbid(unsafe_code)]
use sqlx::{PgPool, Row};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_discussions::repositories::text_issues::{
    CreatedIssue, IssueError, PgTextIssues, StatusMutation,
};

#[tokio::test]
#[ignore = "disposable PostgreSQL 18 tor_ci with migrations"]
async fn e_12_pg_reader_submission_owner_queue_and_status_audit() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never write to a live database");
    let pool = PgPool::connect(&url).await.unwrap();
    let store = PgTextIssues::new(pool.clone());
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Reader','$argon2id$fixture') RETURNING id",
    ).bind(format!("e12-reader-{nonce}@example.invalid"))
        .fetch_one(&pool).await.unwrap();
    let author: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Author','$argon2id$fixture') RETURNING id",
    ).bind(format!("e12-author-{nonce}@example.invalid"))
        .fetch_one(&pool).await.unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','E12 Book','public',$2) RETURNING id",
    ).bind(format!("e12:{nonce}")).bind(author).fetch_one(&pool).await.unwrap();
    let free: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'free','public','free',0) RETURNING id",
    ).bind(book).fetch_one(&pool).await.unwrap();
    let paid: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'paid','public','paid',100) RETURNING id",
    ).bind(book).fetch_one(&pool).await.unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    ).bind(free).bind("Слово слово😀 слово").fetch_one(&pool).await.unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'PRIVATE-PAID-TEXT') RETURNING id",
    ).bind(paid).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES ($1,$2),($3,$4)")
        .bind(free).bind(version).bind(paid).bind(paid_version).execute(&pool).await.unwrap();
    let result = store.create_free(reader,free,version,1,0,5,"Слово","слово","typo").await.unwrap();
    let issue_id = match result { CreatedIssue::Added(id) => id, other => panic!("{other:?}") };
    assert_eq!(
        store.create_free(reader,free,version,1,0,5,"Слово","слово","typo").await.unwrap(),
        CreatedIssue::Replayed(issue_id)
    );
    // Wrong offset/quote, absent version and paid edition must fail closed.
    assert_eq!(store.create_free(reader,free,version,1,6,11,"Слово","fix","typo").await,
        Err(IssueError::Invalid));
    assert_eq!(store.create_free(reader,free,version+100000,1,0,5,"Слово","fix","typo").await,
        Err(IssueError::NotVisible));
    assert_eq!(store.create_free(reader,paid,paid_version,1,0,7,"PRIVATE","fix","typo").await,
        Err(IssueError::NotVisible));
    assert!(store.author_page(reader,book,None,None).await.unwrap().is_none());
    let page = store.author_page(author,book,Some("new"),None).await.unwrap().unwrap();
    assert_eq!(page.issues.len(),1);
    assert_eq!(page.issues[0].id,issue_id);
    assert_eq!(page.issues[0].version_id,version);
    assert_eq!(store.update_status(reader,book,issue_id,"fixed",None).await.unwrap(),
        StatusMutation::NotFound);
    assert_eq!(store.update_status(author,book,issue_id,"in_review",None).await.unwrap(),
        StatusMutation::Changed);
    assert_eq!(store.update_status(author,book,issue_id,"fixed",Some("Создана новая редакция")).await.unwrap(),
        StatusMutation::Changed);
    assert_eq!(store.update_status(author,book,issue_id,"fixed",Some("Создана новая редакция")).await.unwrap(),
        StatusMutation::Replayed);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM text_issue_status_events WHERE issue_id=$1",
    ).bind(issue_id).fetch_one(&pool).await.unwrap();
    assert_eq!(events,2);
    let unmodified: String = sqlx::query(
        "SELECT body_markdown FROM chapter_versions WHERE id=$1"
    ).bind(version).fetch_one(&pool).await.unwrap().try_get("body_markdown").unwrap();
    assert_eq!(unmodified,"Слово слово😀 слово");
    sqlx::query("DELETE FROM books WHERE id=$1").bind(book).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1 OR id=$2")
        .bind(reader).bind(author).execute(&pool).await.unwrap();
    pool.close().await;
}
