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
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never mutate production DB");
    sqlx::query(
        "INSERT INTO books (source_identity,target_language,title,visibility)
         VALUES ('ci:f03-en','en','TOR English Search','public')
         ON CONFLICT (source_identity,target_language)
         DO UPDATE SET title=EXCLUDED.title",
    )
    .execute(&pool)
    .await
    .unwrap();
    let search = PgSearch::new(pool);
    let english = search
        .search_filtered("TOR", Some("EN"), None, 100)
        .await
        .unwrap();
    assert!(!english.items.is_empty());
    assert!(english.items.iter().all(|item| item.language == "en"));
    let russian = search
        .search_filtered("TOR", Some("ru"), None, 100)
        .await
        .unwrap();
    assert!(russian.items.iter().any(|item| item.id == 1));
    assert!(russian.items.iter().all(|item| item.language == "ru"));
    assert_eq!(
        search.search_filtered("TOR", Some("../ru"), None, 20).await,
        Err(SearchError::Invalid)
    );
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL with public/hidden/held fixtures"]
async fn f_03_facet_search_masks_hidden_author_and_genre() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never modify production");
    let rows = [
        (
            "ci:f03-meta:visible",
            "Фэнтези рассвет",
            "Жюль",
            "public",
            false,
        ),
        (
            "ci:f03-meta:hidden",
            "Фэнтези закрыто",
            "Жюль",
            "draft",
            false,
        ),
        ("ci:f03-meta:held", "Фэнтези блок", "Жюль", "public", true),
    ];
    for (identity, title, author, visibility, held) in rows {
        sqlx::query(
            "INSERT INTO books(source_identity,target_language,title,author_name,genres,visibility,admin_hold)
             VALUES ($1,'ru',$2,$3,ARRAY['фэнтези','приключения'],$4,$5)
             ON CONFLICT (source_identity,target_language)
             DO UPDATE SET title=EXCLUDED.title,author_name=EXCLUDED.author_name,
                           genres=EXCLUDED.genres,visibility=EXCLUDED.visibility,
                           admin_hold=EXCLUDED.admin_hold",
        )
        .bind(identity)
        .bind(title)
        .bind(author)
        .bind(visibility)
        .bind(held)
        .execute(&pool)
        .await
        .unwrap();
    }
    let search = PgSearch::new(pool);
    let by_author = search
        .search_faceted("Жюль", Some("ru"), None, None, 30)
        .await
        .unwrap();
    assert!(by_author
        .items
        .iter()
        .any(|item| item.title == "Фэнтези рассвет"));
    assert!(!by_author
        .items
        .iter()
        .any(|item| item.title.contains("закрыто") || item.title.contains("блок")));
    let by_genre = search
        .search_faceted("", None, Some(" ФЭНТЕЗИ "), None, 30)
        .await
        .unwrap();
    assert!(by_genre
        .items
        .iter()
        .any(|item| item.title == "Фэнтези рассвет"));
    assert!(!by_genre
        .items
        .iter()
        .any(|item| item.title.contains("закрыто") || item.title.contains("блок")));
    assert_eq!(
        search
            .search_faceted("", None, Some("неизвестный жанр"), None, 30)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    assert_eq!(
        search
            .search_faceted("Жюль", None, Some("неизвестный жанр"), None, 30)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_03_revocation_removes_author_genre_and_title_from_public_search() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(db, "tor_ci", "never alter production books");

    // Dedicated identity: this test checks the published -> draft/hold/deleted
    // transitions on the *same* row, without deleting shared fixtures.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books
         (source_identity,target_language,title,author_name,genres,visibility,admin_hold,is_deleted)
         VALUES ('ci:f03:revocation','ru','Отзывная книга','Уникальныйавтор',
                 ARRAY['уникальныйжанр'],'draft',FALSE,FALSE)
         ON CONFLICT (source_identity,target_language)
         DO UPDATE SET visibility='draft',admin_hold=FALSE,is_deleted=FALSE,
                       title=EXCLUDED.title,author_name=EXCLUDED.author_name,genres=EXCLUDED.genres
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let search = PgSearch::new(pool.clone());

    // Check three independent discovery dimensions. None may reveal a draft.
    for (term, genre) in [
        ("Отзывная", None),
        ("Уникальныйавтор", None),
        ("", Some("уникальныйжанр")),
    ] {
        let found = search
            .search_faceted(term, Some("ru"), genre, None, 100)
            .await
            .unwrap();
        assert!(!found.items.iter().any(|book| book.id == id as u64));
    }

    sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let published = search
        .search_faceted("", Some("ru"), Some("уникальныйжанр"), None, 100)
        .await
        .unwrap();
    assert!(published.items.iter().any(|book| book.id == id as u64));

    // A hold must revoke existing search results immediately, with no stale
    // index-only result leaking author metadata or even the book identity.
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let held = search
        .search_faceted("Уникальныйавтор", None, None, None, 100)
        .await
        .unwrap();
    assert!(!held.items.iter().any(|book| book.id == id as u64));

    sqlx::query("UPDATE books SET admin_hold=FALSE,visibility='draft' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let withdrawn = search
        .search_faceted("Отзывная", None, None, None, 100)
        .await
        .unwrap();
    assert!(!withdrawn.items.iter().any(|book| book.id == id as u64));

    sqlx::query("UPDATE books SET visibility='public',is_deleted=TRUE WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let deleted = search
        .search_faceted("", None, Some("уникальныйжанр"), None, 100)
        .await
        .unwrap();
    assert!(!deleted.items.iter().any(|book| book.id == id as u64));
}
