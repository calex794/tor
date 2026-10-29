//! E-02: a narrow, real PostgreSQL test for public book comments only.
use tor_discussions::repositories::public_book_comments::{CreateResult, PgPublicBookComments};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn e_02_public_book_visibility_and_replay() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PG URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users (email_normalized,display_name,password_hash)
         VALUES ('e02-public-comment@example.invalid','E02','$argon2id$fixture') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:e02:comment','ru','E02 Book','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgPublicBookComments::new(pool.clone());
    let command_key = "k".repeat(43);
    let hash = [3_u8; 32];
    let body = "Первый ||секрет @hidden_user|| после";
    let first = repo.create(user, book, body, &command_key, &hash).await;
    let id = match first {
        CreateResult::Created(id) => id,
        other => panic!("first write was {other:?}"),
    };
    assert_eq!(
        repo.create(user, book, body, &command_key, &hash).await,
        CreateResult::Replayed(id)
    );
    assert_eq!(
        repo.create(user, book, "Чужой текст", &command_key, &hash)
            .await,
        CreateResult::Conflict
    );
    let comments = repo.list(book, None, 50).await.unwrap().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, id);
    assert_eq!(comments[0].body, "Первый ||секрет @hidden_user|| после");
    assert_eq!(
        tor_discussions::domain::spoilers::delivery_preview(&comments[0].body, 240),
        "Первый [спойлер] после"
    );
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(repo.list(book, None, 50).await.unwrap(), None);
    assert_eq!(
        repo.create(user, book, "Нельзя", &"n".repeat(43), &[4_u8; 32])
            .await,
        CreateResult::NotFound
    );
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn e_02_e03_replies_bind_parent_target_and_concurrent_replay() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PG URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('e03-replies@example.invalid','E03','$argon2id$fixture') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:e03:replies','ru','Replies','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let other_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:e03:other','ru','Other','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgPublicBookComments::new(pool.clone());
    let parent = match repo
        .create(user, book, "Parent", &"a".repeat(43), &[1; 32])
        .await
    {
        CreateResult::Created(id) => id,
        other => panic!("{other:?}"),
    };
    let key = "b".repeat(43);
    let (a, b) = tokio::join!(
        repo.create_with_parent(user, book, "Reply", &key, &[2; 32], Some(parent)),
        repo.create_with_parent(user, book, "Reply", &key, &[2; 32], Some(parent))
    );
    let id = match (&a, &b) {
        (CreateResult::Created(id), CreateResult::Replayed(replay))
        | (CreateResult::Replayed(replay), CreateResult::Created(id))
            if id == replay =>
        {
            *id
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        repo.create(user, book, "Reply", &key, &[2; 32]).await,
        CreateResult::Conflict
    );
    assert_eq!(
        repo.create_with_parent(
            user,
            other_book,
            "No",
            &"c".repeat(43),
            &[3; 32],
            Some(parent)
        )
        .await,
        CreateResult::NotFound
    );
    assert_eq!(
        repo.create_with_parent(user, book, "No", &"c".repeat(43), &[3; 32], Some(i64::MAX))
            .await,
        CreateResult::NotFound
    );
    let listed = repo.list(book, None, 25).await.unwrap().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].parent_id, Some(parent));
    assert_eq!(listed[0].id, id);
    // Replies can themselves receive replies; no parallel thread store.
    assert!(matches!(
        repo.create_with_parent(user, book, "Nested", &"d".repeat(43), &[4; 32], Some(id))
            .await,
        CreateResult::Created(_)
    ));
    for visibility in ["hidden", "deleted", "review"] {
        sqlx::query("UPDATE discussion_comments SET visibility=$1 WHERE id=$2")
            .bind(visibility)
            .bind(parent)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            repo.create_with_parent(user, book, "No", &"e".repeat(43), &[5; 32], Some(parent))
                .await,
            CreateResult::NotFound
        );
    }
    // A writer waiting for a moderation transaction must recheck its committed state.
    sqlx::query("UPDATE discussion_comments SET visibility='visible' WHERE id=$1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    let mut moderation = pool.begin().await.unwrap();
    sqlx::query("UPDATE discussion_comments SET visibility='hidden' WHERE id=$1")
        .bind(parent)
        .execute(&mut *moderation)
        .await
        .unwrap();
    let writer_repo = repo.clone();
    let writer = tokio::spawn(async move {
        writer_repo
            .create_with_parent(
                user,
                book,
                "Racing",
                &"f".repeat(43),
                &[6; 32],
                Some(parent),
            )
            .await
    });
    moderation.commit().await.unwrap();
    assert_eq!(writer.await.unwrap(), CreateResult::NotFound);
    // Delete deepest children first: canonical parent FK remains intact.
    sqlx::query("DELETE FROM discussion_comments WHERE parent_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM discussion_comments WHERE parent_id=$1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id IN ($1,$2)")
        .bind(book)
        .bind(other_book)
        .execute(&pool)
        .await
        .unwrap();
}
