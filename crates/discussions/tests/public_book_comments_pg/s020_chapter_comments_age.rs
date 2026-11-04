//! S020/P0 #486: live 18+/21+ controls on free-edition comments.
use sqlx::PgPool;
use tor_discussions::repositories::public_chapter_comments::{
    CreateResult, NewChapterComment, PgPublicChapterComments,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn s020_free_chapter_comments_recheck_live_age_for_read_and_write() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "do not use production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 free comment reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("s020-free-comment-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let digest = [29_u8; 32];
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,now()+INTERVAL '1 hour')",
    )
    .bind(actor)
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','S020 free comments','public','basic') RETURNING id",
    )
    .bind(format!("ci:s020:free-comments:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Free','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Free S020 body') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)",
    )
    .bind(chapter)
    .bind(version)
    .execute(&pool)
    .await
    .unwrap();
    let repo = PgPublicChapterComments::new(pool.clone());
    let first_key = "a".repeat(43);
    let first = NewChapterComment {
        chapter_id: chapter,
        version_id: version,
        body: "Existing free discussion",
        command_key: &first_key,
        payload_hash: &[1; 32],
        parent_id: None,
        anchor: None,
    };
    assert!(matches!(
        repo.create_free(actor, first).await,
        CreateResult::Created(_)
    ));
    assert_eq!(
        repo.list_free(chapter, version, None, 20)
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1,
    );

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(repo.list_free(chapter, version, None, 20).await.unwrap(), None);
    assert_eq!(
        repo.list_free_for_session(chapter, version, None, 20, Some(&digest))
            .await
            .unwrap(),
        None
    );
    let denied_key = "b".repeat(43);
    assert_eq!(
        repo.create_free(
            actor,
            NewChapterComment {
                command_key: &denied_key,
                body: "Denied adult",
                payload_hash: &[2; 32],
                ..first
            },
        )
        .await,
        CreateResult::NotFound,
    );

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.list_free_for_session(chapter, version, None, 20, Some(&digest))
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1,
    );
    let allowed_key = "c".repeat(43);
    assert!(matches!(
        repo.create_free(
            actor,
            NewChapterComment {
                command_key: &allowed_key,
                body: "Adult permitted",
                payload_hash: &[3; 32],
                ..first
            },
        )
        .await,
        CreateResult::Created(_),
    ));
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.list_free_for_session(chapter, version, None, 20, Some(&digest))
            .await
            .unwrap(),
        None,
    );
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.list_free_for_session(chapter, version, None, 20, Some(&digest))
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        2,
    );
    let pro_key = "d".repeat(43);
    assert!(matches!(
        repo.create_free(
            actor,
            NewChapterComment {
                command_key: &pro_key,
                body: "Pro adult permitted",
                payload_hash: &[4; 32],
                ..first
            },
        )
        .await,
        CreateResult::Created(_),
    ));
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.list_free_for_session(chapter, version, None, 20, Some(&digest))
            .await
            .unwrap(),
        None,
    );
    let revoked_key = "e".repeat(43);
    assert_eq!(
        repo.create_free(
            actor,
            NewChapterComment {
                command_key: &revoked_key,
                body: "Revoked grant",
                payload_hash: &[5; 32],
                ..first
            },
        )
        .await,
        CreateResult::NotFound,
    );
    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(repo.list_free(chapter, version, None, 20).await.unwrap(), None);

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.list_free_for_session(chapter, version, None, 20, Some(&digest))
            .await
            .unwrap(),
        None,
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discussion_comments WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 3, "rejected age requests must not create comments");
}
