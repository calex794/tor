//! S020/E-02: real disposable PostgreSQL, Book-scoped comment age gate.
use sqlx::PgPool;
use tor_discussions::repositories::public_book_comments::{CreateResult, PgPublicBookComments};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn e_02_s020_book_comments_live_age_and_direct_post_gate() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = PgPool::connect(&url).await.unwrap();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s020-e02-actor@example.invalid','Actor','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let writer: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s020-e02-writer@example.invalid','Writer E02','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let digest = [17_u8; 32];
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
        "INSERT INTO books(source_identity,target_language,title,visibility,visibility_status,access_level)
         VALUES('ci:s020:e02-age','ru','E02 age book','public','active','basic')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgPublicBookComments::new(pool.clone());
    assert!(matches!(
        repo.create(writer, book, "Existing body", &"a".repeat(43), &[1; 32])
            .await,
        CreateResult::Created(_)
    ));
    assert_eq!(repo.list(book, None, 20).await.unwrap().unwrap().len(), 1);

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(repo.list(book, None, 20).await.unwrap(), None);
    assert_eq!(
        repo.list_for_session(book, None, 20, Some(&digest))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        repo.create(actor, book, "Denied", &"b".repeat(43), &[2; 32])
            .await,
        CreateResult::NotFound
    );
    assert!(repo
        .mention_candidates(actor, book, "Wri")
        .await
        .unwrap()
        .is_empty());

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let (title, rows) = repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(title, "E02 age book");
    assert_eq!(rows[0].body, "Existing body");
    assert_eq!(
        repo.mention_candidates(actor, book, "Wri").await.unwrap()[0].user_id,
        writer
    );
    assert!(matches!(
        repo.create(actor, book, "Allowed body", &"c".repeat(43), &[3; 32])
            .await,
        CreateResult::Created(_)
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
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_none());
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)
         ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_some());
    assert!(!repo
        .mention_candidates(actor, book, "Wri")
        .await
        .unwrap()
        .is_empty());
    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.create(actor, book, "Revoked", &"d".repeat(43), &[4; 32])
            .await,
        CreateResult::NotFound
    );

    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'moderator',TRUE)
         ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_none());
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)
         ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_some());
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_none());

    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_none());
    sqlx::query(
        "UPDATE books SET access_level='basic',lifecycle_status='pending_deletion' WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_some());
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .list_for_session(book, None, 20, Some(&digest))
        .await
        .unwrap()
        .is_none());

    sqlx::query("DELETE FROM discussion_comments WHERE book_id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM user_level_grants WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM account_roles WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id IN ($1,$2)")
        .bind(actor)
        .bind(writer)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
}
