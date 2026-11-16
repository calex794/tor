//! S019/R1: an unavailable PostgreSQL pool cannot claim a book or source identity.
use super::*;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn b_01_closed_pool_cannot_claim_book_and_retry_creates_one_private_draft() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG test URL");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    let witness = PgPool::connect(&url).await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized, display_name, password_hash)
         VALUES($1, 'S019 outage owner', '$argon2id$fixture') RETURNING id",
    )
    .bind(format!(
        "b01-outage-{}@example.invalid",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
    .fetch_one(&witness)
    .await
    .unwrap();
    let source = format!("tor-ci:s019:outage:{owner}");
    let title = format!("S019 PG outage draft {owner}");
    let key = "q".repeat(43);
    let draft = new_book(&source, "ru", &title, &key).unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let offline = PgAuthoring::new(pool.clone());

    // Close only the author's test-local pool, not the shared CI database.
    pool.close().await;
    assert_eq!(
        offline.create_owned_book(owner, &draft).await,
        Err(AuthorError::Unavailable)
    );
    let before: (i64, i64) = sqlx::query_as(
        "SELECT
          (SELECT count(*) FROM books WHERE source_identity=$1 AND target_language='ru'),
          (SELECT count(*) FROM book_create_commands
           WHERE user_id=$2 AND command_key=$3)",
    )
    .bind(&source)
    .bind(owner)
    .bind(&key)
    .fetch_one(&witness)
    .await
    .unwrap();
    assert_eq!(before, (0, 0), "no book or command may survive the outage");

    let healthy = PgAuthoring::new(witness.clone());
    let book_id = healthy.create_owned_book(owner, &draft).await.unwrap();
    assert_eq!(
        healthy.create_owned_book(owner, &draft).await.unwrap(),
        book_id,
        "retry must return the same committed identity"
    );
    let after: (i64, i64) = sqlx::query_as(
        "SELECT
          (SELECT count(*) FROM books WHERE source_identity=$1 AND target_language='ru'),
          (SELECT count(*) FROM book_create_commands
           WHERE user_id=$2 AND command_key=$3)",
    )
    .bind(&source)
    .bind(owner)
    .bind(&key)
    .fetch_one(&witness)
    .await
    .unwrap();
    assert_eq!(after, (1, 1), "one draft and one immutable command receipt");
    let visibility: String = sqlx::query_scalar("SELECT visibility FROM books WHERE id=$1")
        .bind(book_id)
        .fetch_one(&witness)
        .await
        .unwrap();
    assert_eq!(visibility, "draft", "retry must not publish a new book");
    witness.close().await;
}
