//! BR-060 owner-only two-phase ordinary deletion, disposable PG runtime.
use sqlx::{PgPool, Row};
use tor_books::repositories::postgres::PgAuthoring;
use tor_books::services::authoring::AuthorError;

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn c08_owner_step_up_cas_public_deindex_and_immutable_audit() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let hash = "$argon2id$verified-through-web-only-test";
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Owner C08',$2) RETURNING id",
    )
    .bind(format!("owner-c08-{nonce}@example.invalid"))
    .bind(hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Stranger C08',$2) RETURNING id",
    )
    .bind(format!("stranger-c08-{nonce}@example.invalid"))
    .bind(hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut owner_token = [0_u8; 32];
    owner_token[..16].copy_from_slice(&nonce.to_le_bytes());
    owner_token[16..].copy_from_slice(&nonce.to_le_bytes());
    let mut stranger_token = owner_token;
    stranger_token[0] ^= 0xaa;
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,now()+interval '1 hour'),($3,$4,now()+interval '1 hour')",
    )
    .bind(owner)
    .bind(owner_token.as_slice())
    .bind(stranger)
    .bind(stranger_token.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           visibility,access_level)
         VALUES($1,'ru','Owner deletion', $2, 'public','basic') RETURNING id",
    )
    .bind(format!("c08:ordinary-delete:{nonce}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
         VALUES($1,1,'Free V1','public','free') RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let edition: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'reader owns V1') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(edition)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reader_library_items(user_id,book_id) VALUES($1,$2)")
        .bind(stranger)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
         VALUES($1,$2,$3)",
    )
    .bind(stranger)
    .bind(chapter)
    .bind(edition)
    .execute(&pool)
    .await
    .unwrap();

    let repo = PgAuthoring::new(pool.clone());
    assert!(repo
        .ordinary_deletion_view(stranger, book)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.advance_ordinary_deletion(stranger, book, 1, "start", &stranger_token, hash)
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.advance_ordinary_deletion(owner, book, 1, "complete", &owner_token, hash)
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        repo.advance_ordinary_deletion(owner, book, 1, "start", &owner_token, "$argon2id$wrong")
            .await,
        Err(AuthorError::Missing)
    );

    assert_eq!(
        repo.advance_ordinary_deletion(owner, book, 1, "start", &owner_token, hash)
            .await,
        Ok(2)
    );
    assert_eq!(
        repo.advance_ordinary_deletion(owner, book, 1, "start", &owner_token, hash)
            .await,
        Err(AuthorError::Conflict)
    );
    let pending = repo
        .ordinary_deletion_view(owner, book)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "pending_deletion");
    assert_eq!(pending.revision, 2);
    let vis: String = sqlx::query_scalar("SELECT visibility FROM books WHERE id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(vis, "draft");
    assert_eq!(
        repo.advance_ordinary_deletion(owner, book, 2, "complete", &owner_token, hash)
            .await,
        Ok(3)
    );
    let final_state = repo
        .ordinary_deletion_view(owner, book)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_state.status, "ordinary_deleted");
    assert_eq!(final_state.revision, 3);
    let row = sqlx::query(
        "SELECT is_deleted,(ordinary_deleted_at IS NOT NULL) AS finished,visibility FROM books WHERE id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!row.get::<bool, _>("is_deleted"));
    assert!(row.get::<bool, _>("finished"));
    assert_eq!(row.get::<String, _>("visibility"), "draft");
    let saved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reader_saved_free_chapters
         WHERE user_id=$1 AND chapter_id=$2 AND version_id=$3",
    )
    .bind(stranger)
    .bind(chapter)
    .bind(edition)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved, 1);
    let events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM book_ordinary_deletion_events WHERE book_id=$1")
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 2);
    assert!(
        sqlx::query("DELETE FROM book_ordinary_deletion_events WHERE book_id=$1")
            .bind(book)
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(
        repo.advance_ordinary_deletion(owner, book, 3, "start", &owner_token, hash)
            .await,
        Err(AuthorError::Conflict)
    );

    // A revoked session is never sufficient to mutate even for a real owner.
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(owner_token.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.advance_ordinary_deletion(owner, book, 3, "complete", &owner_token, hash)
            .await,
        Err(AuthorError::Missing)
    );
    pool.close().await;
}
