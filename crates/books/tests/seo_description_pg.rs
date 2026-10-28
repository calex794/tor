use sqlx::{PgPool, Row};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, with_book_description, AuthorError},
};

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn seo02_author_create_is_idempotent_and_description_is_persisted() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"), "never modify production");
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'SEO Author','$argon2id$test') RETURNING id",
    ).bind(format!("seo02-{stamp}@example.invalid")).fetch_one(&pool).await.unwrap();
    let identity = format!("ci:seo02:{stamp}");
    let book = with_book_description(
        new_book(&identity,"ru","Книга",&"s".repeat(43)).unwrap(),
        "  Реальная аннотация\r\nВторая строка ",
    ).unwrap();
    let repo = PgAuthoring::new(pool.clone());
    let id = repo.create_owned_book(user,&book).await.unwrap();
    assert_eq!(repo.create_owned_book(user,&book).await,Ok(id));
    let row = sqlx::query("SELECT description,visibility FROM books WHERE id=$1")
        .bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<String,_>("description"),"Реальная аннотация\nВторая строка");
    assert_eq!(row.get::<String,_>("visibility"),"draft");
    let changed = with_book_description(
        new_book(&identity,"ru","Книга",&"s".repeat(43)).unwrap(),
        "Другая аннотация",
    ).unwrap();
    assert_eq!(repo.create_owned_book(user,&changed).await,Err(AuthorError::Conflict));
}
