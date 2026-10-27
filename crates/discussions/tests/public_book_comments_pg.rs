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
    let first = repo.create(user, book, "Первый", &command_key, &hash).await;
    let id = match first {
        CreateResult::Created(id) => id,
        other => panic!("first write was {other:?}"),
    };
    assert_eq!(
        repo.create(user, book, "Первый", &command_key, &hash).await,
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
    assert_eq!(comments[0].body, "Первый");
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
