//! E-12 integration: immutable version, idempotent proposals, author ACL.
#![forbid(unsafe_code)]
use sqlx::{PgPool, Row};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_discussions::repositories::text_issues::{
    CreatedIssue, IssueError, NewTextIssue, PgTextIssues, StatusMutation,
};

#[tokio::test]
#[ignore = "disposable PostgreSQL 18 tor_ci with migrations"]
async fn e_12_pg_reader_submission_owner_queue_and_status_audit() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never write to a live database");
    let pool = PgPool::connect(&url).await.unwrap();
    let store = PgTextIssues::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("e12-reader-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let author: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Author','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("e12-author-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','E12 Book','public',$2) RETURNING id",
    )
    .bind(format!("e12:{nonce}"))
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();
    let free: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'free','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'paid','public','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    )
    .bind(free)
    .bind("Слово слово😀 слово")
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'PRIVATE-PAID-TEXT') RETURNING id",
    )
    .bind(paid)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES ($1,$2),($3,$4)")
        .bind(free)
        .bind(version)
        .bind(paid)
        .bind(paid_version)
        .execute(&pool)
        .await
        .unwrap();
    let result = store
        .create_free(
            reader,
            NewTextIssue {
                chapter_id: free,
                version_id: version,
                block_index: 1,
                selection_start: 0,
                selection_end: 5,
                quote: "Слово",
                suggestion: "слово",
                issue_type: "typo",
            },
        )
        .await
        .unwrap();
    let issue_id = match result {
        CreatedIssue::Added(id) => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        store
            .create_free(
                reader,
                NewTextIssue {
                    chapter_id: free,
                    version_id: version,
                    block_index: 1,
                    selection_start: 0,
                    selection_end: 5,
                    quote: "Слово",
                    suggestion: "слово",
                    issue_type: "typo",
                },
            )
            .await
            .unwrap(),
        CreatedIssue::Replayed(issue_id)
    );
    // Wrong offset/quote, absent version and paid edition must fail closed.
    assert_eq!(
        store
            .create_free(
                reader,
                NewTextIssue {
                    chapter_id: free,
                    version_id: version,
                    block_index: 1,
                    selection_start: 6,
                    selection_end: 11,
                    quote: "Слово",
                    suggestion: "fix",
                    issue_type: "typo",
                },
            )
            .await,
        Err(IssueError::Invalid)
    );
    assert_eq!(
        store
            .create_free(
                reader,
                NewTextIssue {
                    chapter_id: free,
                    version_id: version + 100000,
                    block_index: 1,
                    selection_start: 0,
                    selection_end: 5,
                    quote: "Слово",
                    suggestion: "fix",
                    issue_type: "typo",
                },
            )
            .await,
        Err(IssueError::NotVisible)
    );
    assert_eq!(
        store
            .create_free(
                reader,
                NewTextIssue {
                    chapter_id: paid,
                    version_id: paid_version,
                    block_index: 1,
                    selection_start: 0,
                    selection_end: 7,
                    quote: "PRIVATE",
                    suggestion: "fix",
                    issue_type: "typo",
                },
            )
            .await,
        Err(IssueError::NotVisible)
    );
    assert!(store
        .author_page(reader, book, None, None)
        .await
        .unwrap()
        .is_none());
    let page = store
        .author_page(author, book, Some("new"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.issues.len(), 1);
    assert_eq!(page.issues[0].id, issue_id);
    assert_eq!(page.issues[0].version_id, version);
    assert!(page.can_edit_chapter);
    let translator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Translator','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("e12-translator-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let moderator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Moderator','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("e12-moderator-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    for (actor, role) in [(translator, "translator"), (moderator, "moderator")] {
        sqlx::query("INSERT INTO book_members(book_id,user_id,role) VALUES ($1,$2,$3)")
            .bind(book)
            .bind(actor)
            .bind(role)
            .execute(&pool)
            .await
            .unwrap();
        let team_page = store
            .author_page(actor, book, None, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(team_page.issues.len(), 1);
        assert!(
            !team_page.can_edit_chapter,
            "{role} must not get the editor link"
        );
    }
    sqlx::query("UPDATE book_members SET is_active=FALSE WHERE book_id=$1 AND user_id=$2")
        .bind(book)
        .bind(translator)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store
        .author_page(translator, book, None, None)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .update_status(translator, book, issue_id, "fixed", None)
            .await
            .unwrap(),
        StatusMutation::NotFound
    );
    assert_eq!(
        store
            .update_status(reader, book, issue_id, "fixed", None)
            .await
            .unwrap(),
        StatusMutation::NotFound
    );
    assert_eq!(
        store
            .update_status(author, book, issue_id, "in_review", None)
            .await
            .unwrap(),
        StatusMutation::Changed
    );
    assert_eq!(
        store
            .update_status(
                author,
                book,
                issue_id,
                "fixed",
                Some("Создана новая редакция")
            )
            .await,
        Err(IssueError::Conflict)
    );
    let revised_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Слово исправлено😀 слово') RETURNING id",
    )
    .bind(free)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(free)
        .bind(revised_version)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .update_status(
                author,
                book,
                issue_id,
                "fixed",
                Some("Создана новая редакция")
            )
            .await
            .unwrap(),
        StatusMutation::Changed
    );
    let resolved: Option<i64> =
        sqlx::query_scalar("SELECT resolved_version_id FROM text_issues WHERE id=$1")
            .bind(issue_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(resolved, Some(revised_version));
    let audited: Option<i64> = sqlx::query_scalar(
        "SELECT resolved_version_id FROM text_issue_status_events
         WHERE issue_id=$1 AND new_status='fixed'",
    )
    .bind(issue_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, Some(revised_version));
    assert_eq!(
        store
            .update_status(
                author,
                book,
                issue_id,
                "fixed",
                Some("Создана новая редакция")
            )
            .await
            .unwrap(),
        StatusMutation::Replayed
    );
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM text_issue_status_events WHERE issue_id=$1")
            .bind(issue_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 2);
    let unmodified: String = sqlx::query("SELECT body_markdown FROM chapter_versions WHERE id=$1")
        .bind(version)
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get("body_markdown")
        .unwrap();
    assert_eq!(unmodified, "Слово слово😀 слово");
    // CI uses a disposable tor_ci database. Keep its fixture until the entire
    // database is dropped: B-10 intentionally prohibits deleting immutable
    // revenue-share history, even when its owner/book was test-created.
    pool.close().await;
}
