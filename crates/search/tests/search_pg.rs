use sqlx::{PgPool, Row};
use tor_search::{PgSearch, SearchError};

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_03_public_books_only_and_stable_cursor() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never modify live data");
    let row = sqlx::query(
        "INSERT INTO books (source_identity,target_language,title,visibility)
         VALUES ('ci:f03','ru','TOR Search Next','public')
         ON CONFLICT(source_identity,target_language)
         DO UPDATE SET title=EXCLUDED.title
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let second: i64 = row.try_get("id").unwrap();
    let search = PgSearch::new(pool);
    let first = search.search("TOR", None, 1).await.unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].id, 1);
    assert_eq!(first.next_cursor.as_deref(), Some("1"));
    let following = search.search("TOR", Some("1"), 50).await.unwrap();
    assert!(following.items.iter().any(|item| item.id == second as u64));
    assert!(!following
        .items
        .iter()
        .any(|item| item.id == 2 || item.id == 3));
    assert!(search
        .search("' OR 1=1 --", None, 50)
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_03_language_filter_and_param_binding() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(database, "tor_ci", "never mutate production DB");
    sqlx::query(
        "INSERT INTO books (source_identity,target_language,title,visibility)
         VALUES ('ci:f03-en','en','TOR English Search','public')
         ON CONFLICT (source_identity,target_language)
         DO UPDATE SET title=EXCLUDED.title",
    ).execute(&pool).await.unwrap();
    let search = PgSearch::new(pool);
    let english = search.search_filtered("TOR", Some("EN"), None, 100).await.unwrap();
    assert!(!english.items.is_empty());
    assert!(english.items.iter().all(|item| item.language == "en"));
    let russian = search.search_filtered("TOR", Some("ru"), None, 100).await.unwrap();
    assert!(russian.items.iter().any(|item| item.id == 1));
    assert!(russian.items.iter().all(|item| item.language == "ru"));
    assert_eq!(search.search_filtered("TOR", Some("../ru"), None, 20).await, Err(SearchError::Invalid));
}
