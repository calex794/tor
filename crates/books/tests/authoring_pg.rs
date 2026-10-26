//! B-01/B-02: a real owner creates one draft, protected against retries.
use sqlx::{PgPool, Row};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, AuthorError},
};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn b_01_owner_draft_is_private_and_idempotent() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never run on production");
    let pool = PgPool::connect(&url).await.expect("PG connect");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized, display_name, password_hash)
         VALUES('b01_owner@tor-ci.example','B01 Owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("fixture");
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized, display_name, password_hash)
         VALUES('b01_other@tor-ci.example','B01 Other','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("fixture");
    // Explicit fixture book IDs do not advance PostgreSQL's identity sequence.
    // Correct only the disposable tor_ci test database, not application logic.
    sqlx::query(
        "SELECT setval(pg_get_serial_sequence('books','id'),
             GREATEST((SELECT COALESCE(MAX(id), 1) FROM books), 1), true)",
    )
    .execute(&pool)
    .await
    .expect("advance book ID sequence past CI fixture");
    let author = PgAuthoring::new(pool.clone());
    let key = "b".repeat(43);
    let identity = format!("tor-ci:b01:{user}");
    let draft = new_book(&identity, "ru", "Первый черновик", &key).unwrap();
    let book_id = author.create_owned_book(user, &draft).await.unwrap();
    assert_eq!(
        author.create_owned_book(user, &draft).await.unwrap(),
        book_id
    );
    assert_eq!(
        author
            .owned_book(user, book_id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "Первый черновик"
    );
    assert!(author.owned_book(other, book_id).await.unwrap().is_none());
    let visible: Option<i64> =
        sqlx::query_scalar("SELECT id FROM books WHERE id=$1 AND visibility='public'")
            .bind(book_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(visible.is_none(), "new books must stay drafts");
    let changed = new_book(&identity, "ru", "Подменённый заголовок", &key).unwrap();
    assert_eq!(
        author.create_owned_book(user, &changed).await,
        Err(AuthorError::Conflict)
    );
    let distinct_key = "c".repeat(43);
    let duplicate = new_book(&identity, "ru", "Первый черновик", &distinct_key).unwrap();
    assert_eq!(
        author.create_owned_book(user, &duplicate).await,
        Err(AuthorError::Conflict),
        "canonical identity must be unique"
    );
    let row = sqlx::query("SELECT owner_user_id,visibility FROM books WHERE id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("owner_user_id"), user);
    assert_eq!(row.get::<String, _>("visibility"), "draft");
    pool.close().await;
}
